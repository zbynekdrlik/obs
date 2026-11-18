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

#[test]
fn the_status_line_prints_this_window_s_decoder_opens() {
    let src = repo(SOURCE);
    let f = function(&src, "static void jxs_log_status(");
    assert!(
        f.contains("decoder_opens=%u"),
        "obs#86: the jxs status line has no decoder_opens= field:\n{f}"
    );
    assert!(
        f.contains("(unsigned)(now.decoder_opens - prev.decoder_opens)"),
        "obs#86: decoder_opens is not this window's delta (now - prev):\n{f}"
    );
    // The field is the last of the format, and its argument is the last of the call.
    let fmt_at = f.find("decoder_opens=%u").unwrap();
    let arg_at = f.find("now.decoder_opens - prev.decoder_opens").unwrap();
    assert!(
        f[fmt_at..].find("fec_repaired=").is_none() && f[..arg_at].contains("now.fec_repaired"),
        "obs#86: decoder_opens's format field and its argument are out of order:\n{f}"
    );
}
