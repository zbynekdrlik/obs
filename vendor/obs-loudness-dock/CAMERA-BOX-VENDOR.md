# obs-loudness-dock, vendored for camera-box (camera-box#1436)

## Upstream

- Repository: https://github.com/norihiro/obs-loudness-dock (GPL-2.0, see `LICENSE`).
- Version: tag `0.5.1`. The annotated tag object is `670588e6ad1a7beb18384fa1a438b302042de61f`, which points
  at commit `6f853ddd64a877fef7e1f74b752f3f59ca8bb3eb`.
- `deps/libebur128` is upstream's git submodule: https://github.com/jiixyj/libebur128 (MIT, see
  `deps/libebur128/COPYING`) at commit `67b33abe1558160ed76ada1322329b0e9e058b02` (the 1.2.6 release).
  It is vendored as plain files.
- The import commit carries both trees byte-identical to those commits. Every camera-box change is
  in later commits, so `git diff <import commit> HEAD -- vendor/obs-loudness-dock` shows exactly our diff.

## Why it is vendored

The stream OBS operator reads the stream output level in LUFS (owner, 4.11.2026). The Windows stream
OBS had a loudness plugin, and the Linux strih bundle lost it. This dock shows EBU R128 momentary,
short-term and integrated loudness, LRA and true peak.

## Our diff

| file | change |
|---|---|
| `src/loudness-mode.h` | NEW. `LOUDNESS_DOCK_EBUR128_MODE` is upstream's mode plus `EBUR128_MODE_HISTOGRAM`, shared with `tests/loudness-ebur128`. |
| `src/loudness.c` | `init_state` passes `LOUDNESS_DOCK_EBUR128_MODE` straight to `ebur128_init`; the mode adds `EBUR128_MODE_HISTOGRAM`. CI checks that it is the dock's only `ebur128_init` call. Without it, libebur128 keeps every 100 ms block since start, and the integrated/LRA read walks the whole list. That read runs every 48 ms under the mutex that `audio_cb` takes on OBS's audio thread, so the hold grows with uptime: about 3 ms at 8 h and 9 ms at 24 h, against a 21.3 ms audio tick (v0.4.12 deep review). Histogram mode makes the read constant-time, at 0.1 LU resolution: the EBU reference sits on a bin edge and reads within ±0.1 of -23 LUFS. |

The camera-box build settings live in `.github/workflows/linux-genlock.yml`:

- It builds against the genlocked OBS SDK with `-DQT_VERSION=6`.
- Upstream uses a system libebur128 only when pkg-config finds one. The runner has none, so the build
  meters with the vendored one. The staging step fails if `ldd` shows a libebur128. PkgConfig cannot be
  disabled, because Qt6Gui's FindXKB needs it.

## Dock id

`net.nagater.obs-loudness-dock..main`, with two dots: `ID_PREFIX` in `CMakeLists.txt` ends with `.`,
and `src/plugin-main.c` appends `.main`. OBS uses it as the dock's Qt objectName. camera-box's
`scripts/obs_dock_state.py` seeds that dock open on the stream role. The staging step fails if the
`.so` does not carry that string.

## Tests

- `tests/loudness-ebur128` feeds the vendored libebur128 the EBU Tech 3341 reference signal: a 1 kHz
  sine at -23 dBFS stereo at 48 kHz. Momentary, short-term and integrated loudness must read
  -23.0 ± 0.1 LUFS, the true peak -23 ± 0.5 dBTP, and silence -inf. It meters with the dock's own mode
  (`src/loudness-mode.h`), checks that mode has `EBUR128_MODE_HISTOGRAM`, and checks LRA reads 0 LU.
- The staging step checks that the dock exports `obs_module_load`, that its locale is staged, and that it
  carries the dock id.
