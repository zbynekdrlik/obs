---
paths:
  - "vstack.pin"
  - "vendor/distroav/src/jxs-*"
  - "tests/vendored/tests/jxs_*.rs"
  - "tests/jxs-output-retry/**"
---

# JPEG XS / the vstack pin

- obs moves the vstack pin (vstack ships the JPEG XS crates; camera-box asks for the move). A move touches three places together:
  - `vstack.pin`: the tag and the full commit;
  - `tests/vendored/Cargo.toml`'s `rig-core` tag;
  - `cargo update -p rig-core` in tests/vendored.

  The `vstack_pin_and_rig_core_tag_agree` gate (obs#62) fails when they differ. Before pushing, run the whole vendored suite with `VSTACK_SRC` = a `git clone --depth 1 --branch <tag>` of vstack (obs#81: it caught the rig-core tag).
- Read the C ABI headers' diff (`jxs/send-ffi/include/camera_box_jxs_send.h`, `jxs/recv-ffi/include/camera_box_jxs.h`) in `gh api repos/zbynekdrlik/vstack/compare/<old>...<new>`. obs never checks the ABI version for equality; `jxs-source.cpp` only logs `cbjxs_abi_version()`. A new pace source code needs its word in `cb_jxs_out_pace_source` (`jxs-output-decide.h`), checked in tests/jxs-output-retry. Example: v0.1.12's 5 = `frame-spread`.
- A status-line field is gated by running the shipped function, not by a text match. `tests/vendored/tests/jxs_decoder_opens_86.rs` lifts `jxs_log_status` into a C++ harness with `obs_log` as a `printf` macro, so `-Wformat=2 -Werror` catches a format/argument mismatch. It compiles against the pinned header and asserts the printed window deltas.
- Verify a vstack release in the bundle: `BUNDLE_MANIFEST.json`'s `vstack_tag` / `vstack_commit`, and `strings` on `distroav.so` for a new status word.
