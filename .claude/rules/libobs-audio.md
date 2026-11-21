---
paths:
  - "vendor/obs-studio/libobs/obs-audio.c"
  - "vendor/obs-studio/libobs/obs-scene.c"
  - "vendor/obs-studio/libobs/obs-source-transition.c"
  - "vendor/obs-studio/libobs/obs-internal.h"
  - "tests/c/genlock_audio_mix_guard_1381_harness.c"
  - "tests/audio-short-ticks/**"
---

# libobs audio mixer (the fork)

- `tests/vendored/tests/genlock_audio_mix_guard_1381.rs` LIFTS the shipped `obs-audio.c` bytes: the functions from `ignore_audio` to `release_audio_sources` and the `audio_callback` blocks. It compiles them with `-Werror` inside `tests/c/genlock_audio_mix_guard_1381_harness.c`, which has its own stub `struct obs_source` and its own stub helpers. A new `obs_source` field or libobs call in a lifted range must also go into that stub. Run the whole vendored suite after any `obs-audio.c` edit: a local libobs build does not see the harness (obs#94: `audio_short_tick_mark` / `os_atomic_inc_long` broke two 1425 tests in CI).
- The mix leaves a source out of a tick with no log: the root loop, a scene's child loop and a transition's child skip any source that is `audio_pending`. A scene returns early when EVERY child is pending, and a transition returns at its `min_ts` gate when both children are. `process_audio_source_tick` sets pending when the source's buffer is short of a tick. The mix guard and `ignore_audio` can render a source twice in one tick. A buffering wait (`buffering_wait_ticks`) mixes nothing.
- obs#94: `obs_source_get_audio_short_ticks()` counts the mixing ticks a mix member sat out. The skip points mark the tick (`obs_source_audio_mix_skipped`), and `audio_callback` counts each mark once inside the mixing block. Under OBS's default dynamic buffering a starved source grows the mix's buffering instead of sitting out. Without a video (`obs_reset_video`) no view mixes audio, so no source is a member. A libobs audio test that needs a mix exclusion therefore runs the box FIXED plan (HOME `.camera-box/genlock-audio-buffering-ms`) and a 64x64 video (`tests/audio-short-ticks`).
- Build and run a libobs C test locally against `vendor/obs-studio/build_ubuntu`. Run `ninja libobs libobs-opengl`, then gcc with `-I libobs -I build_ubuntu/config -I build_ubuntu/libobs -L build_ubuntu/libobs -lobs`. Then `LD_LIBRARY_PATH=build_ubuntu/libobs:build_ubuntu/libobs-opengl xvfb-run -a <test> libobs/data/`. The build tree's `libobsConfig.cmake` does not load for find_package.
- obs#96: the STAMP-basis ASRC books a placement jump only after its 60 s lock (`ASRC_REGRESSION_LOCK_SPAN_S` captures the setpoint), and only a CHANGE beyond the band held for >= 1 s and 8 packets (a steady offset is absorbed by the 10 s EMA). A test of a booking waits past the lock: `tests/audio-short-ticks` steps a source's stamps +30 ms at 75 s. Each booking logs `asrc-booking: source '<name>' jumps= booked_ms= owed_ms= ... err_ms= target_ms= ema_ms= appended= frames= audio_ts_age_ms= buffered_ms= actual_minus_intended_ms= intended_minus_raw_ms= raw_age_ms= basis=` (at most 600 a source).
