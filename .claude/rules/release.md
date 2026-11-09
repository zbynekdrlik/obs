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

# libobs source tests (no video)

- `obs_startup` needs an X display: run under `xvfb-run -a`, as the texture-encode test does.
- Without `obs_reset_video`/audio, `obs_wait_for_destroy_queue` returns at once and does nothing. A released source is destroyed later on libobs's destruction thread. Wait for the source's "destroy" signal, with a bound (`tests/av-sync-measure-hold`).
- libobs lists a removed source until it is destroyed. `obs_get_source_by_name` returns it, and a new source with that name gets renamed ("mbc 2"). Check `obs_source_removed`.
