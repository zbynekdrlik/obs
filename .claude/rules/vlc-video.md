---
paths:
  - "vendor/obs-studio/plugins/vlc-video/**"
  - "tests/vendored/tests/vlc_*.rs"
---

# vlc-video (vendored)

- The libvlc instance is created with `--aout=adummy` (obs#78). VLC 3's `libvlc_media_player_new` opens the instance's default audio output at creation (`lib/media_player.c:748`, `input_resource_GetAout`), before `libvlc_audio_set_callbacks` moves the player to `amem,none` (`media_player.c:1300-1302`). Every `vlc_source`, even a disabled one with an empty playlist, creates a player. On Windows the default output is `mmdevice`, and VLC 3.0.24's `libmmdevice_plugin.dll` crashed OBS on resolume-pp ~3 s after launch. OBS takes VLC's audio only through the callbacks, so never drop the argument.
- `tests/vendored/tests/vlc_no_mmdevice_78.rs` lifts the shipped `load_libvlc` into a C harness with a `libvlc_new_` stub that prints its argv. It reuses the `genlock_audio_ingest_lift` helpers, so the test declares `mod genlock_audio_pairing_lift;` too.
- `tests/vendored/tests/vlc_audio_through_amem_80.rs` runs the lifted `load_libvlc` against real libvlc 3 (`-lvlc`; CI apt `libvlc-dev vlc-plugin-base`) and plays a generated WAV through `libvlc_audio_set_callbacks`: the proof that `--aout=adummy` still hands OBS the audio (obs#80). VLC delivers ahead of real time (0.5 s of audio in ~0.1 s); a mutant without the callbacks fails after the 5 s wait.
- The vendored-tests crate needs `VSTACK_SRC` for two gates (`qpc_drift_books_a_fleet_date_step_1372`, `the_crates_the_obs_build_links_never_depend_on_the_appliance`). They fail locally without it; CI sets it.
