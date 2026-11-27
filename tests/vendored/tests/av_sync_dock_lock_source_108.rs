//! obs#108 — the dock's LOCK-CORRECT reads the latency pin of the source the program shows.
//!
//! stream-snv, v0.4.63, 27.11.2026: `LOCK-CORRECT SUGGESTED genlock_latency_ms_src 1120 -> 1145ms`.
//! 1120 ms is the pin of 'NDI 2ME PGM', which the PRO scene hides. The program comes from
//! 'JXS PGM src' (pin 1019 ms). The dock read one hard-coded name (`CAMERA_BOX_LOCK_SOURCE_NAME`)
//! whatever the program showed.
//!
//! The fix: `camerabox::cb_dock_lock_pick_source` (camera-box-audio.hpp) picks the first of
//! `CB_DOCK_LOCK_SOURCE_NAMES` that exists AND is active (shown in the program). It picks none
//! when no candidate is active. The glue (sync-test-output-audio.cpp) resolves the candidates with
//! `obs_source_active`, reads and applies that source only, and names it in its log lines.
//!
//! The pick is checked by running it (a C++ harness against the real header). The glue builds only
//! against the OBS SDK (linux-genlock.yml), so here it is checked as text, like the other dock
//! anchors.

use std::path::PathBuf;
use std::process::Command;

#[allow(dead_code)]
#[path = "support/av_sync_dock_output.rs"]
mod av_sync_dock_output;

fn manifest_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn squish(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

const HARNESS_CPP: &str = r#"
#include "camera-box-audio.hpp"
#include <cstdio>
#include <cstring>

using namespace camerabox;

static int pick(bool jxs_exists, bool jxs_active, bool ndi_exists, bool ndi_active)
{
	CbDockLockCandidate c[CB_DOCK_LOCK_SOURCE_COUNT];
	c[0].exists = jxs_exists;
	c[0].active = jxs_active;
	c[1].exists = ndi_exists;
	c[1].active = ndi_active;
	return cb_dock_lock_pick_source(c, CB_DOCK_LOCK_SOURCE_COUNT);
}

int main()
{
	std::printf("COUNT %zu\n", CB_DOCK_LOCK_SOURCE_COUNT);
	for (size_t i = 0; i < CB_DOCK_LOCK_SOURCE_COUNT; i++)
		std::printf("NAME %zu '%s'\n", i, CB_DOCK_LOCK_SOURCE_NAMES[i]);
	/* stream-snv 27.11: both exist, the program shows JXS, NDI is hidden */
	std::printf("SNV %d\n", pick(true, true, true, false));
	/* the program switched back to the NDI feed */
	std::printf("NDI_SHOWN %d\n", pick(true, false, true, true));
	/* a box with only the NDI program source */
	std::printf("NDI_ONLY %d\n", pick(false, false, true, true));
	/* both shown: the first candidate */
	std::printf("BOTH %d\n", pick(true, true, true, true));
	/* none shown: no lock source, no suggestion */
	std::printf("NONE_SHOWN %d\n", pick(true, false, true, false));
	/* strih: neither exists */
	std::printf("ABSENT %d\n", pick(false, false, false, false));
	/* an empty candidate list */
	std::printf("EMPTY %d\n", cb_dock_lock_pick_source(nullptr, 0));
	return 0;
}
"#;

fn run_harness() -> String {
    let src_dir = manifest_dir().join("vendor/av-sync-dock/src");
    let work = std::env::temp_dir().join(format!(
        "av_sync_dock_lock_source_108_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&work).expect("create temp workdir");
    let src = work.join("harness.cpp");
    let bin = work.join("harness");
    std::fs::write(&src, HARNESS_CPP).expect("write C++ harness");
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_string());
    let compile = Command::new(&cxx)
        .args(["-std=c++17", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(&src_dir)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to invoke C++ compiler '{cxx}': {e}"));
    assert!(
        compile.status.success(),
        "obs#108: camera-box-audio.hpp must define CB_DOCK_LOCK_SOURCE_NAMES / \
         CbDockLockCandidate / cb_dock_lock_pick_source:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin).output().expect("run compiled harness");
    let _ = std::fs::remove_dir_all(&work);
    assert!(run.status.success(), "harness exit {:?}", run.status.code());
    String::from_utf8_lossy(&run.stdout).into_owned()
}

#[test]
fn the_lock_source_is_the_candidate_the_program_shows() {
    let out = run_harness();
    let expected = "COUNT 2\n\
                    NAME 0 'JXS PGM src'\n\
                    NAME 1 'NDI 2ME PGM'\n\
                    SNV 0\n\
                    NDI_SHOWN 1\n\
                    NDI_ONLY 1\n\
                    BOTH 0\n\
                    NONE_SHOWN -1\n\
                    ABSENT -1\n\
                    EMPTY -1\n";
    assert_eq!(out, expected, "obs#108: the lock-source pick");
}

#[test]
fn the_glue_reads_and_applies_the_picked_source_and_names_it() {
    let src = squish(&av_sync_dock_output::source());
    assert!(
        !src.contains("CAMERA_BOX_LOCK_SOURCE_NAME"),
        "obs#108: the single hard-coded lock source name must be gone"
    );
    assert!(
        src.contains("obs_source_active("),
        "obs#108: the glue must ask which candidate is shown in the program (obs_source_active)"
    );
    assert!(
        src.contains("camerabox::cb_dock_lock_pick_source("),
        "obs#108: the glue must choose the lock source through cb_dock_lock_pick_source"
    );
    let resolver_calls = src.matches("cb_lock_source_get(").count();
    assert!(
        resolver_calls >= 3,
        "obs#108: the resolver must be defined once and used by BOTH the read and the apply path \
         ({resolver_calls} occurrences)"
    );
    let suggested = src
        .split("av-sync-dock: LOCK-CORRECT SUGGESTED genlock_latency_ms_src %d -> %dms (measured offset=%.1fms)")
        .nth(1)
        .expect("obs#108: the SUGGESTED line keeps its existing tokens first");
    assert!(
        suggested.trim_start().starts_with("source='%s'"),
        "obs#108: the SUGGESTED line must name its source right after the measured offset"
    );
    assert!(
        src.contains("av-sync-dock: LOCK-CORRECT source '%s' (pin %d ms)"),
        "obs#108: a change of lock source must be logged with its name and pin"
    );
    assert!(
        src.contains("av-sync-dock: LOCK-CORRECT unavailable -- no lock source shown in the program"),
        "obs#108: no shown candidate must be logged once"
    );
}
