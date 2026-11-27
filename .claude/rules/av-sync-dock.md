---
paths:
  - "vendor/av-sync-dock/**"
  - "tests/vendored/tests/av_sync_dock_*.rs"
---

# A/V-sync dock (vendored)

- The LOCK-CORRECT lock source is the first of `camerabox::CB_DOCK_LOCK_SOURCE_NAMES` (camera-box-audio.hpp: "JXS PGM src", "NDI 2ME PGM") that exists and is `obs_source_active`, i.e. shown in the program (obs#108).
  - `cb_lock_source_get()` resolves it for the read on the audio decode worker and again for the queued UI-thread write.
  - The write carries the read's index and skips when the program shows another source.
  - A switch drops `cb_offset_cluster` and skips that marker.
  - A pass with no source shown keeps `cb_lock_source_idx`.
  - A new program feed name goes into that list, never into one fixed name.
- The dock's C++ is anchored as TEXT in two places. Change a call or a log line in both:
  - the Rust twins `tests/vendored/tests/av_sync_dock_*.rs`, which read the union through `support/av_sync_dock_output.rs`;
  - the pwsh steps of BOTH `.github/workflows/windows-genlock.yml` and `windows-genlock-fast.yml`. For example, the Write case must contain exactly one `cb_apply_lock_latency_ms(act.new_delay_ms, lock_idx);`.
- A text anchor on a format string: C++ splits long ones over adjacent literals (`"... %d " "-> %dms"`). Match on `squish(src).replace("\" \"", "")`, not on the raw text (obs#108 RED had to be fixed for this).
- The glue builds only against the OBS SDK in linux-genlock.yml. Check it locally first:
  - command: `g++ -std=gnu++17 -fsyntax-only -Wall -Wextra -Werror -I vendor/obs-studio/libobs -I vendor/obs-studio/build_ubuntu/{config,libobs} -I vendor/obs-studio/UI/obs-frontend-api -I vendor/av-sync-dock/src -I vendor/av-sync-dock/deps/quirc/lib vendor/av-sync-dock/src/sync-test-output-audio.cpp`
  - it needs `plugin-macros.generated.h` on the include path;
  - test pure logic in `camera-box-audio.hpp` with a compiled C++ harness, like `av_sync_dock_lock_source_108.rs`.
- Changing a dock log line is a camera-box interface change: its av-step watchdog reads the SUGGESTED lines. Keep the existing tokens first, append new ones, and name the change in the hand-off.
