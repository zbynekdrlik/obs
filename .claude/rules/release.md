---
paths:
  - ".github/workflows/**"
  - "VERSION"
  - "tests/libobs-gpu-encode/**"
---

# Release

- A merge to `main` does NOT build or publish anything. `linux-genlock.yml` runs on a push to `dev`, on `workflow_dispatch`, and on a `v*` tag.
- A version bump touches `VERSION`, `plugins/dante-input/ffi/Cargo.toml` and its `Cargo.lock`. The publish step refuses a tag that differs from `VERSION` (v0.4.15 was lost that way); the FFI job checks VERSION against Cargo.toml.
- Release = an annotated tag on the main merge commit, then push the tag:
  `git tag -a vX.Y.Z <merge-sha> -m "vX.Y.Z: <what>" && git push origin vX.Y.Z`
- The tag run builds the strih bundle and runs the job "Publish the release (tag vX.Y.Z on main)". Find it with `gh run list --json headBranch` where `headBranch == "vX.Y.Z"`.
- Assets: `obs-strih-linux-x86_64-vX.Y.Z.tar.zst` (+ `.sha256`), `obs-headers-vX.Y.Z.tar.gz` (+ `.sha256`), `BUNDLE_MANIFEST.json` (no `.sha256`: hash it yourself), `OBS_RELEASE.txt`.
- camera-box pins the release. Send it the tag, the strih tarball sha256 and the manifest sha256.

# libobs texture-encode test

- `tests/libobs-gpu-encode` builds against the obs-sdk prefix. Run it as `xvfb-run -a build/gpu-encode-test <libobs data dir>/`. The data dir is needed because the SDK prefix does not find the libobs effects.
- Use the kernel TID (`syscall(SYS_gettid)`), not `pthread_self()`: pthread_t values are reused once a thread is joined.
- Wait on the output's "deactivate" signal. "stop" fires before the encoders stop.
- Local libobs SDK for these tests (minutes with ccache): in `vendor/obs-studio`, `cmake -S . -B <b> -G Ninja -DCMAKE_BUILD_TYPE=RelWithDebInfo -DOBS_VERSION_OVERRIDE=32.2.0 -DENABLE_FRONTEND=OFF -DENABLE_PLUGINS=OFF -DENABLE_SCRIPTING=OFF -DENABLE_BROWSER=OFF -DENABLE_WAYLAND=OFF -DCMAKE_C_COMPILER_LAUNCHER=ccache`, build, `cmake --install <b> --prefix <sdk>` (+ `--component Development`). Keep `<b>` out of the scratchpad's lifetime assumptions: a reboot clears `/tmp`. `ENABLE_UI=OFF` or plugins off with the frontend on fail configure (`add_dependencies called with incorrect number of arguments`).
- A test that can hang libobs (a stuck encode thread) must leave with `_exit` instead of `obs_shutdown`, which would join the stuck thread (`bail_if_hung`).

# libobs source tests (no video)

- `obs_startup` needs an X display: run under `xvfb-run -a`, as the texture-encode test does.
- Without `obs_reset_video`/audio, `obs_wait_for_destroy_queue` returns at once and does nothing. A released source is destroyed later on libobs's destruction thread. Wait for the source's "destroy" signal, with a bound (`tests/av-sync-measure-hold`).
- libobs lists a removed source until it is destroyed. `obs_get_source_by_name` returns it, and a new source with that name gets renamed ("mbc 2"). Check `obs_source_removed`.

# libobs tick audit (obs#30)

- `tick_sources` in `libobs/obs-video.c` logs `tick-audit:` for a source's or tick callback's video tick over 20 ms: the source's name and id, or the callback's symbol and module (dladdr; an address on Windows). It logs at most 64 lines a process, then one line. OBS's profiler does not split `tick_sources`, so read these lines to name a start stall. Since v0.4.32 a whole `tick_sources` over 20 ms also logs `tick-audit: tick_sources took …` with its phases (callbacks lock/run, sources lock, list, ticks sum + slowest, releases): v0.4.31 on strih showed a stall no single tick explained. `obs_enum_sources` holds `sources_mutex` across all its callbacks, so a slow enumerator on any thread stalls the video thread. Since v0.4.33 the breakdown ends `by type: <id> X ms (n), …`: the 5 source ids with the largest tick sums (the first 512 sources' times are kept on the stack, summed only for a logged call). A source's first tick runs its deferred `update`, `show`, `activate` and `video_tick` on the graphics thread, so an OBS start pays all sources' first-time work in one tick_sources (130 ms over 66 sources on strih, v0.4.32). Each kind of line has its own 64-line budget. Cost: two clock reads per source and per callback each frame, plus a few per call. `tests/tick-audit` checks them.
- OBS writes `== Profiler Results ==` at exit. A short run's profile is mostly the start: `tick_sources` max vs p99 tells a one-off tick stall from a render one.
- Since v0.4.38 (obs#30) new sources' FIRST deferred updates have a per-`tick_sources` budget (`DEFERRED_UPDATE_BUDGET_MS` 8 in obs-internal.h, the time those updates took this frame): past it, a new source whose first update is pending skips that frame's whole tick in `obs_source_video_tick` (its update still runs before its first `video_tick`), the first of every frame always runs. A running source's later updates and a source with no `update` callback are never held (`first_deferred_update_done`). A video source's `obs_source_update` is deferred to its next tick, so without it an OBS start ran every source's first update in one frame (15 FreeType2 text sources ~130 ms on strih). `tick-audit: new sources' first updates took X ms this frame; N new source(s) wait for a later frame` logs a postponement (its own 64-line budget). `tests/deferred-update` checks it.
- Two releases back to back: wait for the first main proof (`gh workflow run linux-genlock.yml --ref main`) to finish before you dispatch the next. Both runs are on `refs/heads/main` and share the concurrency group, so the second dispatch cancels the first. v0.4.53's proof 44501647531 was cancelled this way; v0.4.54's proof then covered main.
- In a PR body, `Closes #N` closes the ticket on merge even inside a sentence that says not to (PR #90: "Closes #89 is left to the hand-off"). Write `#N` without the keyword when the ticket must stay open for the hand-off.
