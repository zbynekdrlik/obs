//! obs#76: DistroAV's NDI output and filter asked the NDI SDK for their connection count with
//! `send_get_no_connections(sender, 10)` once a second from their raw audio and video callbacks.
//! The second argument is a timeout in ms. With no change in connections the SDK waits that long
//! for one, so OBS's audio thread was held ~10 ms every second. On stream-snv on 22.11 that read as
//! the audio-stall line's out_cb_max_ms p50 10.1 ms ('NDI Main Output'), TX pushes 31-32 ms apart,
//! and ~40 dante-input TX underruns in the LIVE. The count is only polled for a log line, so it
//! must never wait: every call passes a timeout of 0.

use std::fs;
use std::path::Path;

const FILES: [&str; 2] = [
    "vendor/distroav/src/ndi-output.cpp",
    "vendor/distroav/src/ndi-filter.cpp",
];

/// Every `send_get_no_connections` call in `src` with its 1-based line and its last argument.
/// A call is the name followed by optional whitespace and `(`, so a spaced call, a call split over
/// lines and two calls on one line are all found; a mention in a comment is not a call.
fn calls_in(src: &str, rel: &str) -> Vec<(usize, String)> {
    const NAME: &str = "send_get_no_connections";
    let mut out = Vec::new();
    for (at, _) in src.match_indices(NAME) {
        let rest = src[at + NAME.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('(') else {
            continue;
        };
        let line = src[..at].matches('\n').count() + 1;
        let mut depth = 1;
        let end = rest
            .char_indices()
            .find(|&(_, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("obs#76: {rel}:{line}: call has no closing ')'"));
        let timeout = rest[..end].rsplit(',').next().unwrap().trim().to_string();
        out.push((line, timeout));
    }
    out
}

#[test]
fn the_ndi_connection_count_is_polled_without_waiting() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut calls = 0;
    for rel in FILES {
        let src = fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("obs#76: read {rel}: {e}"));
        for (line, timeout) in calls_in(&src, rel) {
            calls += 1;
            assert_eq!(
                timeout, "0",
                "obs#76: {rel}:{line} waits {timeout} ms for an NDI connection on a callback thread"
            );
        }
    }
    assert!(
        calls >= 4,
        "obs#76: expected the 4 connection-count polls, found {calls}"
    );
}

#[test]
fn the_call_scan_finds_spaced_split_and_repeated_calls() {
    let src = "// send_get_no_connections is throttled\n\
               a = x->send_get_no_connections (s, 10);\n\
               b = x->send_get_no_connections(\n    f(s), 0); c = x->send_get_no_connections(s, 7);\n";
    assert_eq!(
        calls_in(src, "probe"),
        vec![
            (2, "10".to_string()),
            (3, "0".to_string()),
            (4, "7".to_string())
        ]
    );
}
