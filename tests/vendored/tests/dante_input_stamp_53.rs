//! obs#53 (camera-box issue 1527): dante-input's audio stamps are dlink's per-block play times
//! (v0.6.132, dstack#295), handed through by the feeder; the plugin never re-anchors them on the
//! system clock. The old `stamp()` counted samples from the first block's arrival, drifted with
//! the ~5.9 ppm media-vs-monotonic gap and re-anchored by ~21 ms once an hour: the stream's A/V
//! stepped by that much (stream-snv, 14.11.2026).

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Code only: `//` and `/* ... */` comments dropped, string and char literals kept whole (a `//`
/// or `/*` inside a literal is code).
fn code(src: &str) -> String {
    let mut out = String::new();
    let mut it = src.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' | '\'' => {
                out.push(c);
                while let Some(d) = it.next() {
                    out.push(d);
                    if d == '\\' {
                        if let Some(e) = it.next() {
                            out.push(e);
                        }
                    } else if d == c {
                        break;
                    }
                }
            }
            '/' if it.peek() == Some(&'/') => {
                for d in it.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if it.peek() == Some(&'*') => {
                it.next();
                let mut prev = ' ';
                for d in it.by_ref() {
                    if prev == '*' && d == '/' {
                        break;
                    }
                    if d == '\n' {
                        out.push('\n');
                    }
                    prev = d;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[test]
fn the_code_reader_keeps_literals_and_drops_comments() {
    let src = "a = \"http://x /* y */\"; // gone\nb = '/'; /* gone\n too */ c";
    assert_eq!(code(src), "a = \"http://x /* y */\"; \nb = '/'; \n c");
}

#[test]
fn the_plugin_stamps_with_the_handed_play_time_and_never_re_anchors() {
    let plugin = code(&read("plugins/dante-input/src/plugin.c"));
    assert!(
        plugin.contains("audio.timestamp = timestamp_ns ? timestamp_ns : os_gettime_ns();"),
        "obs#53: on_audio stamps with the feeder's play time, the system clock only without one"
    );
    for gone in [
        "DANTE_INPUT_RESYNC_NS",
        "next_ts",
        "re-anchored on the system clock",
    ] {
        assert!(
            !plugin.contains(gone),
            "obs#53: plugin.c still re-anchors its stamps (`{gone}`)"
        );
    }
}

#[test]
fn the_c_callback_takes_the_play_time() {
    let header = code(&read("plugins/dante-input/include/dante_input.h"));
    assert!(
        header.contains("uint64_t timestamp_ns"),
        "obs#53: the audio callback hands each chunk's play time"
    );
}
