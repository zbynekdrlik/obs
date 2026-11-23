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
- vstack v0.1.20 (obs#100): receiver ABI 6, `cbjxs_set_decoder_cores`. Each jxs source carries `jxs_decoder_cores` (a cpulist, "" = no call), applied by `jxs_apply_decoder_cores` right after `cbjxs_open`; a changed list reopens (`cb_jxs_reopen_needed`). A new `cb_jxs_config` field needs every aggregate initializer extended (jxs-source.cpp's default config too): `-Wmissing-field-initializers` under `-Werror` breaks the vendored compile gate otherwise. Check it locally with `g++ -std=gnu++17 -fsyntax-only -Wall -Wextra -Werror -I vendor/obs-studio/libobs -I vendor/obs-studio/build_ubuntu/{config,libobs} -I <vstack>/jxs/recv-ffi/include -I vendor/distroav/{src,lib} vendor/distroav/src/jxs-source.cpp`. `jxs_decoder_opens_86` checks ABI >= 5, not an exact version.
- DistroAV's NDI output and filter (obs#76): never pass a non-zero timeout to `send_get_no_connections` from a raw audio/video callback. The SDK waits that many ms for a connection when none arrives, and on OBS's audio thread a 10 ms wait once a second made every audio output late (stream-snv 22.11: `audio-stall #1367` out_cb_max_ms p50 10.1 'NDI Main Output', dante-input TX underruns). `tests/vendored/tests/ndi_no_connections_wait_76.rs` gates every call. The `audio-stall #1367` line's `out_cb_name` / `out_cb_max_ms` names the output that holds the audio thread.
