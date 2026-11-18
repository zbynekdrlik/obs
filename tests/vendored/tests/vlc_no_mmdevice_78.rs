//! obs#78 (camera-box#1441): OBS crashed on resolume-pp ~3 s after launch in VLC 3.0.24's
//! `libmmdevice_plugin.dll` (c0000005), from a DISABLED `vlc_source` with an empty playlist.
//!
//! VLC 3's `libvlc_media_player_new` (and the list player's own player) opens the instance's
//! default audio output at once, before `libvlc_audio_set_callbacks` moves that player to `amem`;
//! on Windows the default is `mmdevice`. OBS takes VLC's audio only through the callbacks, so the
//! vlc-video plugin creates its libvlc instance with the dummy output as the default
//! (`--aout=adummy`): no source, enabled or not, ever loads a system audio output.
//!
//! The gate lifts the shipped `load_libvlc` (`vendor/obs-studio/plugins/vlc-video/
//! vlc-video-plugin.c`) and runs it against a `libvlc_new_` stub that prints the arguments it
//! was given. It FAILS LOUDLY when no C compiler is present.

mod genlock_audio_ingest_lift;
mod genlock_audio_pairing_lift;
use genlock_audio_ingest_lift::{compile_and_run, lift_fn, read};

const PLUGIN: &str = "vendor/obs-studio/plugins/vlc-video/vlc-video-plugin.c";

const HARNESS: &str = r#"
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

typedef struct libvlc_instance_t libvlc_instance_t;
static libvlc_instance_t *libvlc;
static uint64_t time_start;
static int instance;

static libvlc_instance_t *stub_libvlc_new(int argc, const char *const *argv)
{
	printf("argc %d\n", argc);
	for (int i = 0; i < argc; i++)
		printf("arg %s\n", argv[i]);
	return (libvlc_instance_t *)&instance;
}
static libvlc_instance_t *(*libvlc_new_)(int argc, const char *const *argv) = stub_libvlc_new;
static int64_t stub_libvlc_clock(void)
{
	return 1;
}
static int64_t (*libvlc_clock_)(void) = stub_libvlc_clock;
#define LOG_INFO 300
#define blog(level, ...) ((void)(level), printf("log " __VA_ARGS__), printf("\n"))

bool load_libvlc(void);
LIFTED

int main(void)
{
	printf("loaded %d\n", load_libvlc() ? 1 : 0);
	return 0;
}
"#;

#[test]
fn the_libvlc_instance_defaults_to_the_dummy_audio_output_so_no_source_loads_mmdevice() {
    let src = read(PLUGIN);
    let lifted = lift_fn(&src, "bool load_libvlc(void)\n{");
    let out = compile_and_run(&HARNESS.replace("LIFTED", &lifted), "vlc_no_mmdevice_78");
    let args: Vec<&str> = out
        .iter()
        .filter_map(|l| l.strip_prefix("arg "))
        .collect();
    assert!(
        out.iter().any(|l| l == "loaded 1"),
        "obs#78: load_libvlc did not load: {out:?}"
    );
    assert!(
        args.contains(&"--aout=adummy"),
        "obs#78: libvlc_new must get --aout=adummy, or a VLC media player opens the system audio \
         output (mmdevice on Windows: the resolume-pp crash) before OBS's audio callbacks take \
         over; it got {args:?} ({out:?})"
    );
    assert!(
        !args.iter().any(|a| a.starts_with("--aout=") && *a != "--aout=adummy"),
        "obs#78: one default audio output only: {args:?}"
    );
}
