//! Issue 1462 -- the NVENC texture encoder leaked ~154 MiB of GPU memory per StartRecord/StopRecord
//! on the Linux stream-snv (RTX 4060), ~50 cycles to CUDA_ERROR_OUT_OF_MEMORY.
//!
//! The OpenGL path registers libobs's encode textures with `cuGraphicsGLRegisterImage` while
//! `cuda_opengl_encode` holds the graphics context (`obs_enter_graphics`). `nvenc_destroy` runs from
//! `obs_encoder_shutdown` on the output's stop thread with NO GL context current, and
//! `cuda_opengl_free` unregistered them there with every return value ignored. These tests lift the
//! REAL `cuda_opengl_free` out of `nvenc-opengl.c`, compile it against stubbed CUDA + graphics
//! symbols and assert that every unregister runs inside the graphics context and that a failing
//! unregister is reported through the plugin's own `cuda_error_check`.
//!
//! camera-box#1387 (obs PR #10) moved the unregister body into `cuda_opengl_free_task`, which
//! `cuda_opengl_free` queues on the graphics thread (`obs_queue_task(OBS_TASK_GRAPHICS, ..., true)`)
//! or runs inline when the caller already holds graphics (`gs_get_context()`). Both functions are
//! lifted; the queue stub runs the task synchronously, as the graphics thread does with `wait`.
//!
//! Runs under `cargo test` (scratch dir via the `tempfile` dev-dependency, issue 1409).
//! Fails loudly when no C compiler is present.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const NVENC_OPENGL: &str = "vendor/obs-studio/plugins/obs-nvenc/nvenc-opengl.c";
const FREE_SIG: &str = "void cuda_opengl_free(struct nvenc_data *enc)";
const TASK_SIG: &str = "static void cuda_opengl_free_task(void *param)";

fn repo() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// A function, by its signature, through its first top-level closing brace.
fn lift_fn(src: &str, signature: &str) -> String {
    assert_eq!(
        src.matches(signature).count(),
        1,
        "issue 1462: `{signature}` must be defined exactly once"
    );
    let a = src.find(signature).unwrap();
    let b = src[a..]
        .find("\n}\n")
        .unwrap_or_else(|| panic!("issue 1462: `{signature}` does not end"));
    src[a..a + b + 3].to_string()
}

const HARNESS_HEAD: &str = r#"
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>

typedef int CUresult;
#define CUDA_SUCCESS 0
typedef void *CUcontext;
typedef void *CUgraphicsResource;

struct handle_tex {
	unsigned tex_id;
	CUgraphicsResource res_y;
	CUgraphicsResource res_uv;
};

struct nvenc_data {
	CUcontext cu_ctx;
	struct {
		struct handle_tex *array;
		size_t num;
	} input_textures;
};

static int gfx_depth;
static int ctx_depth;
static int fail_unregister_of = -1; /* index of the resource whose unregister fails */

static int fail_push;
static int caller_holds_graphics; /* gs_get_context() non-NULL: the caller is inside graphics */

static void obs_enter_graphics(void) { printf("SEQ enter\n"); gfx_depth++; }
static void obs_leave_graphics(void) { printf("SEQ leave\n"); gfx_depth--; }

static CUresult stub_push(CUcontext c)
{
	(void)c;
	printf("SEQ push\n");
	if (fail_push)
		return 201;
	ctx_depth++;
	return CUDA_SUCCESS;
}
static CUresult stub_pop(CUcontext *c) { (void)c; printf("SEQ pop\n"); ctx_depth--; return CUDA_SUCCESS; }
static CUresult stub_unregister(CUgraphicsResource r)
{
	int idx = (int)(size_t)r;
	printf("UNREG res=%d in_graphics=%d in_ctx=%d\n", idx, gfx_depth > 0, ctx_depth > 0);
	return idx == fail_unregister_of ? 999 : CUDA_SUCCESS;
}

struct cu_fns {
	CUresult (*cuCtxPushCurrent)(CUcontext);
	CUresult (*cuCtxPopCurrent)(CUcontext *);
	CUresult (*cuGraphicsUnregisterResource)(CUgraphicsResource);
};
static struct cu_fns fns = {stub_push, stub_pop, stub_unregister};
static struct cu_fns *cu = &fns;

typedef struct graphics_subsystem graphics_t;
static graphics_t *gs_get_context(void) { return caller_holds_graphics ? (graphics_t *)&fns : NULL; }

enum obs_task_type { OBS_TASK_UI, OBS_TASK_GRAPHICS, OBS_TASK_AUDIO, OBS_TASK_DESTROY };
typedef void (*obs_task_t)(void *param);
/* the graphics thread runs the task; with wait, the caller returns after it */
static void obs_queue_task(enum obs_task_type type, obs_task_t task, void *param, bool wait)
{
	printf("QUEUE graphics=%d wait=%d\n", type == OBS_TASK_GRAPHICS, wait);
	task(param);
}

static bool cuda_error_check(struct nvenc_data *enc, CUresult res, const char *func, const char *call)
{
	(void)enc;
	if (res == CUDA_SUCCESS)
		return true;
	printf("ERROR func=%s call=%s res=%d\n", func, call, res);
	return false;
}
"#;

const HARNESS_MAIN: &str = r#"
int main(void)
{
	struct handle_tex texs[2] = {{1, (void *)1, (void *)2}, {2, (void *)3, (void *)4}};
	struct nvenc_data enc = {(void *)0x10, {texs, 2}};

	printf("CASE ok\n");
	cuda_opengl_free(&enc);
	printf("DEPTH gfx=%d ctx=%d\n", gfx_depth, ctx_depth);

	printf("CASE fail\n");
	fail_unregister_of = 3;
	cuda_opengl_free(&enc);
	printf("DEPTH gfx=%d ctx=%d\n", gfx_depth, ctx_depth);

	printf("CASE pushfail\n");
	fail_unregister_of = -1;
	fail_push = 1;
	cuda_opengl_free(&enc);
	printf("DEPTH gfx=%d ctx=%d\n", gfx_depth, ctx_depth);
	fail_push = 0;

	printf("CASE notex\n");
	struct nvenc_data empty = {(void *)0x10, {texs, 0}};
	cuda_opengl_free(&empty);
	printf("DEPTH gfx=%d ctx=%d\n", gfx_depth, ctx_depth);

	printf("CASE noctx\n");
	struct nvenc_data none = {NULL, {texs, 2}};
	int before = gfx_depth;
	cuda_opengl_free(&none);
	printf("NOCTX gfx_touched=%d\n", gfx_depth != before);

	printf("CASE holder\n");
	caller_holds_graphics = 1;
	cuda_opengl_free(&enc);
	printf("DEPTH gfx=%d ctx=%d\n", gfx_depth, ctx_depth);
	caller_holds_graphics = 0;
	return 0;
}
"#;

fn run_harness() -> Vec<String> {
    let src =
        fs::read_to_string(repo().join(NVENC_OPENGL)).expect("issue 1462: read nvenc-opengl.c");
    let task = lift_fn(&src, TASK_SIG);
    let body = lift_fn(&src, FREE_SIG);
    let program = format!("{HARNESS_HEAD}\n{task}\n{body}\n{HARNESS_MAIN}");
    let scratch = tempfile::tempdir().expect("issue 1462: scratch dir");
    let dir = scratch.path();
    let c = dir.join("harness.c");
    let bin = dir.join("harness");
    fs::write(&c, program).expect("issue 1462: write harness");
    let out = Command::new("cc")
        .args([
            "-std=gnu11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wno-unused-function",
        ])
        .arg(&c)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("issue 1462: a C compiler (cc) is required -- this gate never skips");
    assert!(
        out.status.success(),
        "issue 1462: the harness does not compile:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin)
        .output()
        .expect("issue 1462: run the harness");
    assert!(run.status.success(), "issue 1462: the harness failed");
    String::from_utf8_lossy(&run.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn case(lines: &[String], name: &str) -> Vec<String> {
    let start = lines
        .iter()
        .position(|l| l == &format!("CASE {name}"))
        .unwrap_or_else(|| panic!("issue 1462: no CASE {name}"));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.starts_with("CASE "))
        .cloned()
        .collect()
}

#[test]
fn every_unregister_runs_inside_the_graphics_context_1462() {
    let lines = run_harness();
    let ok = case(&lines, "ok");
    let unregs: Vec<&String> = ok.iter().filter(|l| l.starts_with("UNREG ")).collect();
    assert_eq!(
        unregs.len(),
        4,
        "issue 1462: both planes of both textures: {ok:?}"
    );
    for l in &unregs {
        assert!(
            l.contains("in_graphics=1"),
            "issue 1462: a GL-interop unregister ran without the GL context current: {l}"
        );
        assert!(
            l.contains("in_ctx=1"),
            "issue 1462: unregister outside the CUDA context: {l}"
        );
    }
    assert!(
        ok.contains(&"DEPTH gfx=0 ctx=0".to_string()),
        "issue 1462: graphics/CUDA context left unbalanced: {ok:?}"
    );
}

#[test]
fn a_failing_unregister_is_reported_and_the_rest_still_run_1462() {
    let lines = run_harness();
    let fail = case(&lines, "fail");
    assert_eq!(
        fail.iter().filter(|l| l.starts_with("UNREG ")).count(),
        4,
        "issue 1462: one failure must not stop the other unregisters: {fail:?}"
    );
    let errors: Vec<&String> = fail.iter().filter(|l| l.starts_with("ERROR ")).collect();
    assert_eq!(
        errors.len(),
        1,
        "issue 1462: the failing unregister must be logged once: {fail:?}"
    );
    assert!(
        errors[0].contains("cuGraphicsUnregisterResource") && errors[0].contains("res=999"),
        "issue 1462: the error line must name the call and its result: {}",
        errors[0]
    );
    assert!(fail.contains(&"DEPTH gfx=0 ctx=0".to_string()));
}

#[test]
fn no_cuda_context_touches_nothing_1462() {
    let lines = run_harness();
    let noctx = case(&lines, "noctx");
    assert!(noctx.iter().all(|l| !l.starts_with("UNREG ")));
    assert!(noctx.contains(&"NOCTX gfx_touched=0".to_string()));
}

/// The CUDA context is pushed before the graphics context is entered and popped after it is left,
/// the same order as `cuda_opengl_encode`, so the two threads never take them in opposite order.
#[test]
fn the_cuda_context_wraps_the_graphics_context_like_the_encoder_1462() {
    let lines = run_harness();
    let ok = case(&lines, "ok");
    let seq: Vec<&str> = ok.iter().filter_map(|l| l.strip_prefix("SEQ ")).collect();
    assert_eq!(
        seq,
        ["push", "enter", "leave", "pop"],
        "issue 1462: lock order must match cuda_opengl_encode: {ok:?}"
    );
}

/// A failing push is reported and nothing runs in the wrong context; graphics stays balanced.
#[test]
fn a_failing_context_push_is_reported_and_unregisters_nothing_1462() {
    let lines = run_harness();
    let pf = case(&lines, "pushfail");
    assert!(
        pf.iter().all(|l| !l.starts_with("UNREG ")),
        "issue 1462: no unregister without the CUDA context: {pf:?}"
    );
    let errors: Vec<&String> = pf.iter().filter(|l| l.starts_with("ERROR ")).collect();
    assert_eq!(
        errors.len(),
        1,
        "issue 1462: the failed push is logged once: {pf:?}"
    );
    assert!(
        errors[0].contains("cuCtxPushCurrent") && errors[0].contains("res=201"),
        "issue 1462: the error names the push: {}",
        errors[0]
    );
    assert!(pf.contains(&"DEPTH gfx=0 ctx=0".to_string()), "{pf:?}");
}

/// An encoder with no registered texture (the CUDA-upload path, a failed create) takes neither
/// context: destroy never waits for the graphics lock for nothing.
#[test]
fn no_registered_texture_takes_no_context_1462() {
    let lines = run_harness();
    let notex = case(&lines, "notex");
    assert!(
        notex.iter().all(|l| !l.starts_with("SEQ ")),
        "issue 1462: nothing to unregister must take no context: {notex:?}"
    );
}

/// camera-box#1387: from a thread that does not hold graphics, the unregister is queued on the
/// graphics thread and the caller waits for it (a free that returned early would race the
/// encoder teardown).
#[test]
fn the_unregister_is_queued_on_the_graphics_thread_and_waited_for_1387() {
    let lines = run_harness();
    let ok = case(&lines, "ok");
    assert_eq!(
        ok.iter().filter(|l| l.starts_with("QUEUE ")).collect::<Vec<_>>(),
        ["QUEUE graphics=1 wait=1"],
        "camera-box#1387: one waited graphics-thread task: {ok:?}"
    );
    let notex = case(&lines, "notex");
    assert!(
        notex.iter().all(|l| !l.starts_with("QUEUE ")),
        "camera-box#1387: nothing to unregister queues nothing: {notex:?}"
    );
}

/// camera-box#1387: a caller already inside graphics runs the task inline (queueing it would wait
/// on a graphics thread that cannot take the lock), still with both contexts and balanced.
#[test]
fn a_caller_holding_graphics_runs_the_unregister_inline_1387() {
    let lines = run_harness();
    let holder = case(&lines, "holder");
    assert!(
        holder.iter().all(|l| !l.starts_with("QUEUE ")),
        "camera-box#1387: a graphics holder must not queue: {holder:?}"
    );
    let unregs: Vec<&String> = holder.iter().filter(|l| l.starts_with("UNREG ")).collect();
    assert_eq!(unregs.len(), 4, "{holder:?}");
    assert!(unregs
        .iter()
        .all(|l| l.contains("in_graphics=1") && l.contains("in_ctx=1")));
    assert!(holder.contains(&"DEPTH gfx=0 ctx=0".to_string()), "{holder:?}");
}
