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

#[test]
fn the_ndi_connection_count_is_polled_without_waiting() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut calls = 0;
    for rel in FILES {
        let src = fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("obs#76: read {rel}: {e}"));
        for (i, line) in src.lines().enumerate() {
            let Some(at) = line.find("send_get_no_connections(") else {
                continue;
            };
            calls += 1;
            let args = &line[at + "send_get_no_connections(".len()..];
            let args = &args[..args
                .find(')')
                .unwrap_or_else(|| panic!("obs#76: {rel}:{}: no ')'", i + 1))];
            let timeout = args.rsplit(',').next().unwrap().trim();
            assert_eq!(
                timeout,
                "0",
                "obs#76: {rel}:{} waits {timeout} ms for an NDI connection on a callback thread: {}",
                i + 1,
                line.trim()
            );
        }
    }
    assert!(
        calls >= 4,
        "obs#76: expected the 4 connection-count polls, found {calls}"
    );
}
