//! obs#86 (vstack v0.1.13, vstack#20): the JPEG XS receiver C ABI 5 reports
//! `cbjxs_stats.decoder_opens`, the decoders opened from a whole frame. Since vstack#20 a bpp swap
//! within the last 2 levels opens none. The jxs source's 5 s status line carries this window's
//! delta (`decoder_opens=`), so a strih box shows whether a swap still costs a decoder open
//! (camera-box#1536).
//!
//! The gate checks both sides. The pinned vstack header (VSTACK_SRC at vstack.pin) declares the
//! field at ABI 5, and `jxs_log_status` (`vendor/distroav/src/jxs-source.cpp`) prints the
//! window's delta, `now.decoder_opens - prev.decoder_opens`, the same way as its other
//! counters.

#[path = "support/vstack.rs"]
mod vstack;

use std::fs;
use std::path::Path;

const SOURCE: &str = "vendor/distroav/src/jxs-source.cpp";
const HEADER: &str = "jxs/recv-ffi/include/camera_box_jxs.h";

fn repo(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("obs#86: read {rel}: {e}"))
}

/// One function of `src` from `signature` to its closing `\n}\n`.
fn function<'a>(src: &'a str, signature: &str) -> &'a str {
    let a = src
        .find(signature)
        .unwrap_or_else(|| panic!("obs#86: `{signature}` not in {SOURCE}"));
    let b = src[a..]
        .find("\n}\n")
        .unwrap_or_else(|| panic!("obs#86: `{signature}` does not end"));
    &src[a..a + b]
}

#[test]
fn the_pinned_receiver_abi_carries_decoder_opens() {
    let header = vstack::read(HEADER);
    assert!(
        header.contains("#define CBJXS_ABI_VERSION 5u"),
        "obs#86: the pinned vstack's receiver ABI is not 5 (vstack.pin moved without obs#86?)"
    );
    assert!(
        header.contains("uint32_t decoder_opens;"),
        "obs#86: cbjxs_stats has no decoder_opens at the pinned vstack"
    );
}

const HARNESS: &str = r#"
#include <cinttypes>
#include <cstdint>
#include <cstdio>
#include "camera_box_jxs.h"
#include "jxs-source-decide.h"

struct jxs_source {
	void *source;
};
static const char *obs_source_get_name(void *) { return "Cam 1"; }
#define LOG_INFO 300
#define obs_log(level, ...) ((void)(level), printf(__VA_ARGS__), printf("\n"))
struct stub_state {
	void publish_window(const void *, uint32_t, uint32_t, uint64_t, uint64_t, uint64_t, uint64_t, int64_t) {}
};
static stub_state state_;
static stub_state &cb_jxs_state() { return state_; }

LIFTED

int main()
{
	jxs_source s{nullptr};
	cbjxs_stats prev{}, now{};
	prev.frames = 1000;
	now.frames = 1250;
	prev.fec_repaired = 2;
	now.fec_repaired = 9;
	prev.decoder_opens = 4;
	now.decoder_opens = 7;
	jxs_log_status(&s, now, prev, 5.0, 1920, 1080, 0);
	/* the u32 counter wraps: the window's delta is still 2 */
	prev.decoder_opens = 0xFFFFFFFFu;
	now.decoder_opens = 1;
	jxs_log_status(&s, now, prev, 5.0, 1920, 1080, 0);
	return 0;
}
"#;

/// The shipped `jxs_log_status`, compiled with `-Wformat=2 -Werror` against the pinned vstack
/// header with `-Wformat-signedness` (a format/argument or signedness mismatch fails the build), run
/// on known counters: `decoder_opens=` sits before `fec_repaired=`, so the line keeps its old tail.
#[test]
fn the_status_line_prints_this_window_s_decoder_opens() {
    let src = repo(SOURCE);
    let f = function(&src, "static void jxs_log_status(");
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("jxs_decoder_opens_86_{}", std::process::id()));
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
        .unwrap_or_else(|e| panic!("obs#86: could not run the C++ compiler `{cxx}` ({e})"));
    assert!(
        out.status.success(),
        "obs#86: the lifted jxs_log_status does not build (a format/argument mismatch?):\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&bin)
        .output()
        .expect("obs#86: the harness did not run");
    assert!(run.status.success(), "obs#86: the harness failed");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "obs#86: two status lines expected:\n{stdout}"
    );
    assert!(
        lines[0].contains(" decoder_opens=3 fec_repaired=7 (issue 1387)"),
        "obs#86: decoder_opens must sit before fec_repaired, the line still ending in `fec_repaired=N (issue 1387)`:\n{}",
        lines[0]
    );
    assert!(
        lines[0].starts_with("jxs-source 'Cam 1': fps=50.00 frames=250 "),
        "obs#86: {}",
        lines[0]
    );
    assert!(
        lines.iter().all(|l| l.ends_with(" (issue 1387)")),
        "obs#86: the line's tail changed:\n{stdout}"
    );
    assert!(
        lines[1].contains(" decoder_opens=2 "),
        "obs#86: a wrapped u32 counter's delta: {}",
        lines[1]
    );
    let _ = fs::remove_dir_all(&dir);
}
