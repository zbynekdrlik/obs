//! obs#80 (review of obs#78): with the libvlc instance's default audio output the dummy
//! (`--aout=adummy`, so no VLC player opens the system output on creation), VLC's audio must still
//! reach OBS through the per-player audio callbacks (`amem`), as `vlcs_create`
//! (`vendor/obs-studio/plugins/vlc-video/vlc-video-source.c`) sets them.
//!
//! The gate lifts the shipped `load_libvlc` (`vlc-video-plugin.c`) and runs it against the real
//! libvlc 3 (`-lvlc`, the system VLC's plugins), then plays a generated half-second WAV through a
//! media player with `libvlc_audio_set_callbacks` + `libvlc_audio_set_format_callbacks` and asserts
//! the play callback received the audio. It FAILS LOUDLY when no C compiler, no libvlc or no VLC
//! plugins are present (CI installs `libvlc-dev` + `vlc-plugin-base`).

mod genlock_audio_ingest_lift;
mod genlock_audio_pairing_lift;
use genlock_audio_ingest_lift::{lift_fn, read};

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const PLUGIN: &str = "vendor/obs-studio/plugins/vlc-video/vlc-video-plugin.c";
const RATE: u32 = 48_000;
const FRAMES: u32 = RATE / 2;

const HARNESS: &str = r#"
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
#include <vlc/vlc.h>

static libvlc_instance_t *libvlc;
static uint64_t time_start;
static libvlc_instance_t *(*libvlc_new_)(int argc, const char *const *argv) = libvlc_new;
static int64_t (*libvlc_clock_)(void) = libvlc_clock;
#define LOG_INFO 300
#define blog(level, ...) ((void)(level), printf("log " __VA_ARGS__), printf("\n"))

bool load_libvlc(void);
LIFTED

static volatile unsigned long played;
static volatile unsigned setups;

static int audio_setup(void **data, char *format, unsigned *rate, unsigned *channels)
{
	(void)data;
	printf("setup %.4s %u %u\n", format, *rate, *channels);
	setups++;
	return 0;
}

static void audio_play(void *data, const void *samples, unsigned count, int64_t pts)
{
	(void)data;
	(void)samples;
	(void)pts;
	played += count;
}

int main(int argc, char **argv)
{
	if (argc != 2)
		return 2;
	if (!load_libvlc()) {
		printf("no libvlc instance\n");
		return 1;
	}
	(void)time_start;
	libvlc_media_player_t *mp = libvlc_media_player_new(libvlc);
	if (!mp) {
		printf("no media player\n");
		return 1;
	}
	libvlc_audio_set_callbacks(mp, audio_play, NULL, NULL, NULL, NULL, NULL);
	libvlc_audio_set_format_callbacks(mp, audio_setup, NULL);
	libvlc_media_t *m = libvlc_media_new_path(libvlc, argv[1]);
	if (!m) {
		printf("no media\n");
		return 1;
	}
	libvlc_media_player_set_media(mp, m);
	libvlc_media_release(m);
	if (libvlc_media_player_play(mp) != 0) {
		printf("play refused\n");
		return 1;
	}
	for (int i = 0; i < 100 && played < 12000; i++)
		usleep(50000);
	libvlc_media_player_stop(mp);
	libvlc_media_player_release(mp);
	libvlc_release(libvlc);
	printf("setups %u\n", setups);
	printf("played %lu\n", played);
	return 0;
}
"#;

/// A mono 16-bit PCM WAV of `frames` frames of a 440 Hz tone at `rate`.
fn wav(frames: u32, rate: u32) -> Vec<u8> {
    let data_len = frames * 2;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for f in 0..frames {
        let x = (2.0 * std::f64::consts::PI * 440.0 * f64::from(f) / f64::from(rate)).sin();
        b.extend_from_slice(&((x * 16_000.0) as i16).to_le_bytes());
    }
    b
}

#[test]
fn vlc_audio_reaches_the_amem_callbacks_with_the_dummy_default_output() {
    let src = read(PLUGIN);
    let lifted = lift_fn(&src, "bool load_libvlc(void)\n{");
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("vlc_audio_through_amem_80_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create the scratch dir");
    let harness = dir.join("harness.c");
    let bin = dir.join("harness.bin");
    let media = dir.join("tone.wav");
    fs::write(&harness, HARNESS.replace("LIFTED", &lifted)).expect("write the harness");
    fs::write(&media, wav(FRAMES, RATE)).expect("write the WAV");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let out = Command::new(&cc)
        .args(["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O1"])
        .arg(&harness)
        .arg("-o")
        .arg(&bin)
        .arg("-lvlc")
        .output()
        .unwrap_or_else(|e| {
            panic!("obs#80: could not run the C compiler `{cc}` ({e}); install one")
        });
    assert!(
        out.status.success(),
        "obs#80: the libvlc harness does not build (libvlc-dev missing?):\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(&bin)
        .arg(&media)
        .output()
        .expect("obs#80: the libvlc harness did not execute");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert!(
        run.status.success(),
        "obs#80: the libvlc harness failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let value = |key: &str| -> u64 {
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| panic!("obs#80: no `{key}` line in:\n{stdout}"))
    };
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("log [vlc-video]: libvlc instance with --aout=adummy")),
        "obs#80: load_libvlc did not create the instance with --aout=adummy:\n{stdout}"
    );
    assert!(
        value("setups ") >= 1,
        "obs#80: the amem format callback never ran (VLC opened no amem output):\n{stdout}"
    );
    assert!(
        value("played ") >= u64::from(FRAMES) / 2,
        "obs#80: VLC's audio did not reach the amem play callback with the dummy default \
         output (the half-second tone is {FRAMES} frames):\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
