//! obs#103 (stream-snv 24./25.11.2026: one dante-input tx_underrun at each stream/recording start
//! or stop; the audio thread's mix ran 28-38 ms in those ticks): the probes that show whether the
//! audio thread waits for `obs->video.mixes_mutex`, and who held it.
//!
//! - `audio_stall_note_order` (obs-audio.c) keeps the window maxima of the wait for the lock and of
//!   the whole render-order build, printed as `mixes_wait_max_ms` / `order_max_ms` on the
//!   audio-stall #1367 line (tests/audio_stall_out_1381.rs pins the line).
//! - `obs_mixes_mutex_note_hold` (obs-video.c) logs a `mixes-mutex:` line when a site held the lock
//!   10 ms or more, at most 600 lines.
//!
//! Both functions are lifted verbatim into a C harness with stub clocks and a stub `blog`, and
//! compiled with -Werror. A C compiler is required: the gate fails loudly without one.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const OBS_AUDIO: &str = "vendor/obs-studio/libobs/obs-audio.c";
const OBS_VIDEO: &str = "vendor/obs-studio/libobs/obs-video.c";
const NOTE_ORDER_SIG: &str = "static void audio_stall_note_order(";
const NOTE_HOLD_SIG: &str = "void obs_mixes_mutex_note_hold(";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("obs#103: read {rel}: {e}"))
}

/// A function, by its signature, through its first top-level closing brace.
fn lift_fn(src: &str, signature: &str) -> String {
    assert_eq!(
        src.matches(signature).count(),
        1,
        "obs#103: `{signature}` must be defined exactly once"
    );
    let a = src.find(signature).unwrap();
    let b = src[a..]
        .find("\n}\n")
        .unwrap_or_else(|| panic!("obs#103: `{signature}` does not end"));
    src[a..a + b + 3].to_string()
}

const HARNESS: &str = r#"
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#define LOG_INFO 300
static long os_atomic_inc_long(volatile long *v) { return ++*v; }
static int lines;
static char last[512];
static void blog(int level, const char *fmt, ...)
{
	(void)level;
	va_list ap;
	va_start(ap, fmt);
	vsnprintf(last, sizeof(last), fmt, ap);
	va_end(ap);
	lines++;
}
void obs_mixes_mutex_note_hold(const char *site, uint64_t locked_ns, uint64_t unlock_ns);
static uint64_t audio_stall_mixes_wait_max_ns = 0;
static uint64_t audio_stall_order_max_ns = 0;
@NOTE_ORDER@
@NOTE_HOLD@
int main(void)
{
	audio_stall_note_order(1000, 1000, 1500000);
	printf("ORDER1 wait=%llu order=%llu\n", (unsigned long long)audio_stall_mixes_wait_max_ns,
	       (unsigned long long)audio_stall_order_max_ns);
	audio_stall_note_order(2000000, 31650000, 31700000);
	audio_stall_note_order(40000000, 40000100, 40300000);
	printf("ORDER2 wait=%llu order=%llu\n", (unsigned long long)audio_stall_mixes_wait_max_ns,
	       (unsigned long long)audio_stall_order_max_ns);

	obs_mixes_mutex_note_hold("short", 100000000, 109999999);
	printf("HOLD_SHORT lines=%d\n", lines);
	obs_mixes_mutex_note_hold("maybe_clear_encoder_core_video_mix", 100000000, 130000000);
	printf("HOLD_LONG lines=%d %s\n", lines, last);
	obs_mixes_mutex_note_hold("clock_back", 100000000, 90000000);
	printf("HOLD_BACK lines=%d\n", lines);
	for (int i = 0; i < 1000; i++)
		obs_mixes_mutex_note_hold("output_frames", 100000000, 200000000);
	printf("HOLD_CAP lines=%d\n", lines);
	return 0;
}
"#;

/// One compile + run per test binary: the tests share it (two compiles into one dir would race).
fn run_harness() -> &'static Vec<String> {
    static TRACE: OnceLock<Vec<String>> = OnceLock::new();
    TRACE.get_or_init(build_and_run)
}

fn build_and_run() -> Vec<String> {
    let src = HARNESS
        .replace("@NOTE_ORDER@", &lift_fn(&read(OBS_AUDIO), NOTE_ORDER_SIG))
        .replace("@NOTE_HOLD@", &lift_fn(&read(OBS_VIDEO), NOTE_HOLD_SIG));
    let dir = std::env::temp_dir().join(format!("mixes-mutex-hold-103-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("obs#103: scratch dir");
    let c = dir.join("harness.c");
    let bin = dir.join("harness");
    fs::write(&c, src).expect("obs#103: write harness");
    let out = Command::new("cc")
        .args(["-std=gnu11", "-Wall", "-Wextra", "-Wformat=2", "-Werror"])
        .arg("-Wno-format-nonliteral")
        .arg(&c)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("obs#103: a C compiler (cc) is required -- this gate never skips");
    let compiled = out.status.success();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let run = if compiled {
        Some(
            Command::new(&bin)
                .output()
                .expect("obs#103: run the harness"),
        )
    } else {
        None
    };
    let _ = fs::remove_dir_all(&dir);
    assert!(compiled, "obs#103: the harness does not compile:\n{stderr}");
    let run = run.unwrap();
    assert!(run.status.success(), "obs#103: the harness failed");
    String::from_utf8_lossy(&run.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_order_probe_keeps_the_window_maxima_of_the_wait_and_the_build_103() {
    let t = run_harness();
    assert_eq!(t[0], "ORDER1 wait=0 order=1499000");
    // a 29.65 ms wait for the lock, then a short tick: the maxima stay
    assert_eq!(t[1], "ORDER2 wait=29650000 order=29700000");
}

#[test]
fn a_hold_of_10_ms_or_more_names_its_site_at_most_600_times_103() {
    let t = run_harness();
    assert_eq!(t[2], "HOLD_SHORT lines=0");
    assert_eq!(
        t[3],
        "HOLD_LONG lines=1 mixes-mutex: maybe_clear_encoder_core_video_mix held \
         obs->video.mixes_mutex 30.0 ms (the audio thread waits for it; obs#103)"
    );
    assert_eq!(t[4], "HOLD_BACK lines=1");
    assert_eq!(t[5], "HOLD_CAP lines=600");
}

/// `text` with every run of whitespace collapsed to one space, so a reflow or a re-indent of the C
/// source does not break the structural checks below.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The audio thread measures the wait right around its one mixes_mutex lock, and every site that
/// frees a mix or renders under that lock reads the clock before its unlock and reports the hold
/// AFTER it (the log write never lengthens the hold the audio thread waits on).
#[test]
fn the_lock_and_its_slow_holders_are_measured_103() {
    let audio = squash(&read(OBS_AUDIO));
    let order = squash(
        "const uint64_t order_start_ns = os_gettime_ns();
         pthread_mutex_lock(&obs->video.mixes_mutex);
         const uint64_t order_locked_ns = os_gettime_ns();",
    );
    assert_eq!(
        audio.matches(&order).count(),
        1,
        "obs#103: the order probe around the lock"
    );
    let note = squash(
        "audio_stall_note_order(order_start_ns, order_locked_ns, os_gettime_ns());
         pthread_mutex_unlock(&obs->video.mixes_mutex);",
    );
    assert_eq!(
        audio.matches(&note).count(),
        1,
        "obs#103: the order note before the unlock"
    );
    for (file, site) in [
        ("vendor/obs-studio/libobs/obs-video.c", "output_frames"),
        (
            "vendor/obs-studio/libobs/obs-encoder.c",
            "maybe_set_up_gpu_rescale",
        ),
        (
            "vendor/obs-studio/libobs/obs-encoder.c",
            "maybe_clear_encoder_core_video_mix",
        ),
        (
            "vendor/obs-studio/libobs/obs-canvas.c",
            "obs_canvas_clear_mix",
        ),
    ] {
        let call = squash(&format!(
            "const uint64_t mixes_unlock_ns = os_gettime_ns();
             pthread_mutex_unlock(&obs->video.mixes_mutex);
             obs_mixes_mutex_note_hold(\"{site}\", mixes_locked_ns, mixes_unlock_ns);"
        ));
        assert_eq!(
            squash(&read(file)).matches(&call).count(),
            1,
            "obs#103: {site} in {file} must read the clock before its mixes_mutex unlock and report after it"
        );
    }
}
