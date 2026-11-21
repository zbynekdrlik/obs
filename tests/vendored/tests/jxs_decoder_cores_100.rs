//! obs#100 (vstack v0.1.20, vstack#30): the JPEG XS receiver C ABI 6 adds
//! `cbjxs_set_decoder_cores(rx, cpulist)`: the decoder's SVT threads (jxd-*) open on those cores,
//! and the thread polling `cbjxs_next_frame` keeps its own. Each jxs source carries the cpulist as
//! its `jxs_decoder_cores` setting (empty = unchanged: no call). The call goes right after
//! `cbjxs_open`, because a decoder already open keeps its threads, so a changed list reopens the
//! receiver.
//!
//! The gate runs the shipped code: the decision helpers in `jxs-source-decide.h` and
//! `jxs_apply_decoder_cores` lifted from `vendor/distroav/src/jxs-source.cpp`. Both are compiled
//! with `-Wformat=2 -Werror` against the pinned vstack header, with a stub
//! `cbjxs_set_decoder_cores` that records each call.

#[path = "support/vstack.rs"]
mod vstack;

use std::fs;
use std::path::Path;

const SOURCE: &str = "vendor/distroav/src/jxs-source.cpp";
const HEADER: &str = "jxs/recv-ffi/include/camera_box_jxs.h";

fn repo(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("obs#100: read {rel}: {e}"))
}

/// One function of `src` from `signature` to its closing `\n}\n`.
fn function<'a>(src: &'a str, signature: &str) -> &'a str {
    let a = src
        .find(signature)
        .unwrap_or_else(|| panic!("obs#100: `{signature}` not in {SOURCE}"));
    let b = src[a..]
        .find("\n}\n")
        .unwrap_or_else(|| panic!("obs#100: `{signature}` does not end"));
    &src[a..a + b]
}

#[test]
fn the_pinned_receiver_abi_declares_set_decoder_cores() {
    let header = vstack::read(HEADER);
    assert!(
        header.contains("#define CBJXS_ABI_VERSION 6u"),
        "obs#100: the pinned vstack's receiver ABI is not 6"
    );
    assert!(
        header.contains("int cbjxs_set_decoder_cores(cbjxs_receiver *rx, const char *cores);"),
        "obs#100: the pinned header does not declare cbjxs_set_decoder_cores"
    );
}

const HARNESS: &str = r#"
#include <cinttypes>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include "camera_box_jxs.h"
#include "jxs-source-decide.h"

#define LOG_WARNING 200
#define LOG_INFO 300
#define obs_log(level, fmt, ...) printf("[%d] " fmt "\n", level, __VA_ARGS__)

struct jxs_source {
	void *source;
};
static const char *obs_source_get_name(void *) { return "Cam 1"; }

/* the stub library: records each call, answers with `next_ret` */
static int calls = 0;
static char last_cores[128];
static int next_ret = 0;
int cbjxs_set_decoder_cores(cbjxs_receiver *, const char *cores)
{
	calls++;
	snprintf(last_cores, sizeof last_cores, "%s", cores ? cores : "(null)");
	return next_ret;
}
size_t cbjxs_take_error(cbjxs_receiver *, char *buf, size_t len)
{
	return (size_t)snprintf(buf, len, "%s", "decoder cores \"x\": not a cpulist");
}

LIFTED

int main()
{
	/* the decision: an empty list is no call; a changed list reopens; a copy truncates safely */
	cb_jxs_config a = {5004, 1, 0, 100, {0}};
	cb_jxs_config b = a;
	if (cb_jxs_decoder_cores_wanted(&a)) return 10;
	if (cb_jxs_reopen_needed(a, b)) return 11;
	cb_jxs_set_decoder_cores(&b, "8-11");
	if (!cb_jxs_decoder_cores_wanted(&b) || strcmp(b.decoder_cores, "8-11") != 0) return 12;
	if (!cb_jxs_reopen_needed(a, b)) return 13;
	char longs[200];
	memset(longs, '7', sizeof longs - 1);
	longs[sizeof longs - 1] = '\0';
	cb_jxs_set_decoder_cores(&b, longs);
	if (strlen(b.decoder_cores) != sizeof b.decoder_cores - 1) return 14;
	cb_jxs_set_decoder_cores(&b, nullptr);
	if (cb_jxs_decoder_cores_wanted(&b)) return 15;

	jxs_source s = {nullptr};
	cbjxs_receiver *rx = reinterpret_cast<cbjxs_receiver *>(&s);
	/* empty: no call, no line */
	jxs_apply_decoder_cores(&s, rx, a);
	if (calls != 0) return 20;
	/* set: one call with the list, an INFO line */
	cb_jxs_set_decoder_cores(&b, "8-11");
	jxs_apply_decoder_cores(&s, rx, b);
	if (calls != 1 || strcmp(last_cores, "8-11") != 0) return 21;
	/* refused: a WARNING naming the library's reason */
	next_ret = -2;
	jxs_apply_decoder_cores(&s, rx, b);
	if (calls != 2) return 22;
	return 0;
}
"#;

#[test]
fn each_source_pins_its_decoder_threads_to_its_cpulist() {
    let src = repo(SOURCE);
    let f = function(&src, "static void jxs_apply_decoder_cores(");
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("jxs_decoder_cores_100_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create the scratch dir");
    let harness = dir.join("harness.cpp");
    let bin = dir.join("harness.bin");
    fs::write(&harness, HARNESS.replace("LIFTED", &format!("{f}\n}}\n")))
        .expect("write the harness");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_string());
    let out = std::process::Command::new(&cxx)
        .args([
            "-std=gnu++17",
            "-Wall",
            "-Wextra",
            "-Wformat=2",
            "-Wformat-signedness",
            "-Werror",
            "-O1",
        ])
        .arg("-I")
        .arg(vstack::path("jxs/recv-ffi/include"))
        .arg("-I")
        .arg(root.join("vendor/distroav/src"))
        .arg(&harness)
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap_or_else(|e| panic!("obs#100: could not run the C++ compiler `{cxx}` ({e})"));
    assert!(
        out.status.success(),
        "obs#100: the decide helpers / the lifted jxs_apply_decoder_cores do not build:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&bin)
        .output()
        .expect("obs#100: the harness did not run");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert!(
        run.status.success(),
        "obs#100: the harness failed (exit {:?}):\n{stdout}",
        run.status.code()
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "obs#100: one INFO and one WARNING line expected:\n{stdout}");
    assert!(
        lines[0].starts_with("[300] jxs-source 'Cam 1': decoder threads on cores 8-11"),
        "obs#100: {}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("[200] jxs-source 'Cam 1': decoder cores \"8-11\" refused (-2)")
            && lines[1].contains("not a cpulist"),
        "obs#100: {}",
        lines[1]
    );
}
