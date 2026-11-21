/* obs#94 (camera-box#1527): libobs counts, per source, the mixing ticks a member of an output mix with
 * its audio started sat out (audio_pending after the tick's last render: its buffer short of the tick;
 * the mix skips it with no log), once a tick in audio_callback. stream-snv runs FIXED 0 ms audio
 * buffering, where such a source is silently left out; dante-input puts this count on its status line
 * (late_to_mix). It counts where the mix skips the source (the root loop, a scene's or a transition's
 * child loop), so a source mixed through a scene (as dante-input is) counts the same. The test runs that plan too: HOME is a temp dir whose .camera-box/genlock-audio-buffering-ms
 * says 0 (the box FIXED plan, the mix never grows). Under OBS's default dynamic buffering a starved
 * source grows the mix's buffering instead (a buffering wait, nothing mixed) and is never left out.
 * Real libobs: sources push 128-frame blocks at 48 kHz, each 45 ms (about two 21.3 ms
 * ticks) before its stamp, from their own threads:
 *
 *   - steady pushes on the mix: no short tick once the mix runs (counted from 3 s in to 8 s; the
 *     first tick after a source's first audio can count one, before the mix has settled on it);
 *   - the pusher on the mix stops for 150 ms and then catches up: its buffer runs dry, and it counts
 *     at least one and at most 12 short ticks (150 ms is ~7 ticks; once a tick, never twice);
 *   - a source outside every mix that stalls the same way counts 0 (rendered for its meters, never
 *     mixed, so never left out);
 *   - a source on the mix that never pushed audio counts 0 (no audio_ts: silent, not short);
 *   - the same steady and stalled pushes from inside a scene on the mix count the same (0; 1..12).
 *
 * The sources are put on output channels of a running video (obs_reset_video 64x64), which makes them
 * members of the output mix: without video no view mixes audio and nothing is mixed.
 *
 *   audio-short-ticks-test <libobs data dir>   (the SDK's share/obs/libobs/, which holds the effects)
 *
 * Exit 0 = every check held. */

#include <obs.h>
#include <util/base.h>
#include <util/platform.h>
#include <util/threading.h>

#include <stdarg.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int failures;

static void expect(bool ok, const char *what)
{
	fprintf(stderr, "%s: %s\n", ok ? "OK" : "FAIL", what);
	if (!ok)
		failures++;
}

static void log_line(int level, const char *fmt, va_list args, void *param)
{
	(void)param;
	if (level > LOG_WARNING)
		return;
	char line[4096];
	vsnprintf(line, sizeof(line), fmt, args);
	fprintf(stderr, "[%d] %s\n", level, line);
}

static const char *pusher_name(void *type_data)
{
	(void)type_data;
	return "short-tick pusher";
}

static void *pusher_create(obs_data_t *settings, obs_source_t *source)
{
	(void)settings;
	(void)source;
	return bzalloc(1);
}

static void pusher_destroy(void *data)
{
	bfree(data);
}

struct run {
	obs_source_t *source;
	/* the push of this block index waits 150 ms first (0 = never) */
	uint64_t stall_at;
	atomic_bool stop;
};

#define BLOCK 128ULL
#define LEAD_NS 45000000ULL

static void *push_loop(void *param)
{
	struct run *r = param;
	float plane[BLOCK];
	for (size_t i = 0; i < BLOCK; i++)
		plane[i] = (float)(i % 32) / 64.0f;
	const uint64_t t0 = os_gettime_ns() + 2 * LEAD_NS;
	for (uint64_t k = 0; !atomic_load(&r->stop); k++) {
		const uint64_t due = t0 + k * BLOCK * 1000000000ULL / 48000ULL;
		if (due > os_gettime_ns() + LEAD_NS)
			os_sleepto_ns(due - LEAD_NS);
		if (r->stall_at && k == r->stall_at)
			os_sleep_ms(150);
		struct obs_source_audio a = {0};
		a.data[0] = (const uint8_t *)plane;
		a.data[1] = (const uint8_t *)plane;
		a.frames = (uint32_t)BLOCK;
		a.speakers = SPEAKERS_STEREO;
		a.format = AUDIO_FORMAT_FLOAT_PLANAR;
		a.samples_per_sec = 48000;
		a.timestamp = due;
		obs_source_output_audio(r->source, &a);
	}
	return NULL;
}

struct counts {
	uint64_t at_3s, at_8s; /* the pusher on the mix */
	uint64_t off_at_3s, off_at_8s; /* the same pusher outside every mix */
	uint64_t silent; /* a source on the mix that never pushed */
};

static bool start_pusher(struct run *r, pthread_t *thread, const char *name, uint64_t stall_at)
{
	r->stall_at = stall_at;
	atomic_init(&r->stop, false);
	r->source = obs_source_create("short_tick_pusher", name, NULL, NULL);
	if (!r->source)
		return false;
	if (pthread_create(thread, NULL, push_loop, r) != 0) {
		obs_source_release(r->source);
		r->source = NULL;
		return false;
	}
	return true;
}

static void stop_pusher(struct run *r, pthread_t thread)
{
	atomic_store(&r->stop, true);
	pthread_join(thread, NULL);
}

/* One case of 8 s: the short ticks at 3 s and at 8 s. */
static bool run_case(const char *name, uint64_t stall_at, bool in_scene, struct counts *c)
{
	struct run mixed = {0}, off = {0};
	pthread_t t_mixed, t_off;
	obs_source_t *quiet = obs_source_create("short_tick_pusher", "never pushes", NULL, NULL);
	const bool ok_mixed = start_pusher(&mixed, &t_mixed, name, stall_at);
	const bool ok_off = start_pusher(&off, &t_off, "outside every mix", stall_at);
	const bool ok = quiet && ok_mixed && ok_off;
	obs_scene_t *scene = NULL;
	if (ok && in_scene) {
		scene = obs_scene_create_private(name);
		if (scene)
			obs_scene_add(scene, mixed.source);
	}
	const bool scene_made = scene != NULL;
	if (ok && (!in_scene || scene)) {
		obs_set_output_source(0, in_scene ? obs_scene_get_source(scene) : mixed.source);
		obs_set_output_source(1, quiet);
		os_sleep_ms(3000);
		c->at_3s = obs_source_get_audio_short_ticks(mixed.source);
		c->off_at_3s = obs_source_get_audio_short_ticks(off.source);
		os_sleep_ms(5000);
		c->at_8s = obs_source_get_audio_short_ticks(mixed.source);
		c->off_at_8s = obs_source_get_audio_short_ticks(off.source);
		c->silent = obs_source_get_audio_short_ticks(quiet);
		obs_set_output_source(0, NULL);
		obs_set_output_source(1, NULL);
	}
	if (ok_mixed)
		stop_pusher(&mixed, t_mixed);
	/* the scene holds its own ref on the pusher: release it first */
	obs_scene_release(scene);
	scene = NULL;
	if (ok_mixed)
		obs_source_release(mixed.source);
	if (ok_off) {
		stop_pusher(&off, t_off);
		obs_source_release(off.source);
	}
	obs_source_release(quiet);
	if (in_scene && ok && !scene_made) {
		expect(false, "the scene is created");
		return false;
	}
	if (!ok) {
		expect(false, "the sources and their pushers start");
		return false;
	}
	fprintf(stderr,
		"%s: on the mix %llu at 3 s, %llu at 8 s; outside every mix %llu, %llu; the silent source %llu\n",
		name, (unsigned long long)c->at_3s, (unsigned long long)c->at_8s, (unsigned long long)c->off_at_3s,
		(unsigned long long)c->off_at_8s, (unsigned long long)c->silent);
	return true;
}

int main(int argc, char **argv)
{
	if (argc != 2) {
		fprintf(stderr, "usage: audio-short-ticks-test <libobs data dir>\n");
		return 2;
	}
	base_set_log_handler(log_line, NULL);
	/* the box FIXED plan at 0 ms, as stream-snv (obs.c genlock_audio_buffering_read_box_ms) */
	const char *tmp = getenv("TMPDIR");
	char home[512];
	char path[600];
	snprintf(home, sizeof(home), "%s/audio-short-ticks-XXXXXX", tmp && *tmp ? tmp : "/tmp");
	if (!mkdtemp(home)) {
		fprintf(stderr, "FAIL: mkdtemp\n");
		return 1;
	}
	snprintf(path, sizeof(path), "%s/.camera-box", home);
	mkdir(path, 0700);
	snprintf(path, sizeof(path), "%s/.camera-box/genlock-audio-buffering-ms", home);
	FILE *f = fopen(path, "w");
	if (!f || fputs("0\n", f) < 0 || fclose(f) != 0) {
		fprintf(stderr, "FAIL: cannot write %s\n", path);
		return 1;
	}
	setenv("HOME", home, 1);
	if (!obs_startup("en-US", NULL, NULL)) {
		fprintf(stderr, "FAIL: obs_startup\n");
		return 1;
	}
	struct obs_audio_info ai = {0};
	ai.samples_per_sec = 48000;
	ai.speakers = SPEAKERS_STEREO;
	expect(obs_reset_audio(&ai), "obs_reset_audio 48 kHz stereo");
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
	obs_add_data_path(argv[1]);
#pragma GCC diagnostic pop
	struct obs_video_info ovi = {0};
	ovi.graphics_module = "libobs-opengl";
	ovi.fps_num = 30;
	ovi.fps_den = 1;
	ovi.base_width = ovi.output_width = 64;
	ovi.base_height = ovi.output_height = 64;
	ovi.output_format = VIDEO_FORMAT_NV12;
	ovi.gpu_conversion = true;
	ovi.colorspace = VIDEO_CS_709;
	ovi.range = VIDEO_RANGE_PARTIAL;
	ovi.scale_type = OBS_SCALE_BICUBIC;
	if (obs_reset_video(&ovi) != OBS_VIDEO_SUCCESS) {
		fprintf(stderr, "FAIL: obs_reset_video 64x64 (the output mix)\n");
		obs_shutdown();
		return 1;
	}

	struct obs_source_info info = {0};
	info.id = "short_tick_pusher";
	info.type = OBS_SOURCE_TYPE_INPUT;
	info.output_flags = OBS_SOURCE_AUDIO;
	info.get_name = pusher_name;
	info.create = pusher_create;
	info.destroy = pusher_destroy;
	obs_register_source(&info);

	expect(obs_source_get_audio_short_ticks(NULL) == 0, "a NULL source reads 0");

	static const struct {
		const char *steady, *stall;
		bool in_scene;
	} places[] = {
		{"steady pushes", "a 150 ms push stall", false},
		{"steady pushes in a scene", "a 150 ms push stall in a scene", true},
	};
	for (size_t i = 0; i < sizeof(places) / sizeof(places[0]); i++) {
		char what[200];
		struct counts c = {0};
		if (run_case(places[i].steady, 0, places[i].in_scene, &c)) {
			snprintf(what, sizeof(what), "%s: no short tick from 3 s to 8 s", places[i].steady);
			expect(c.at_8s == c.at_3s, what);
			snprintf(what, sizeof(what), "%s: a source outside every mix counts none", places[i].steady);
			expect(c.off_at_8s == 0, what);
			snprintf(what, sizeof(what), "%s: a source that never pushed counts none", places[i].steady);
			expect(c.silent == 0, what);
		}
		/* block 1500 = 4 s in: inside the 3-8 s window */
		memset(&c, 0, sizeof(c));
		if (run_case(places[i].stall, 1500, places[i].in_scene, &c)) {
			const uint64_t rise = c.at_8s - c.at_3s;
			snprintf(what, sizeof(what), "%s: the dry buffer counts short ticks", places[i].stall);
			expect(rise >= 1, what);
			snprintf(what, sizeof(what), "%s: at most 12 (~7 ticks dry, once a tick)", places[i].stall);
			expect(rise <= 12, what);
			snprintf(what, sizeof(what), "%s: the same stall outside every mix counts none", places[i].stall);
			expect(c.off_at_8s == 0, what);
			snprintf(what, sizeof(what), "%s: a source that never pushed counts none", places[i].stall);
			expect(c.silent == 0, what);
		}
	}

	obs_shutdown();
	unlink(path);
	snprintf(path, sizeof(path), "%s/.camera-box", home);
	rmdir(path);
	rmdir(home);
	if (failures)
		fprintf(stderr, "%d check(s) failed\n", failures);
	else
		fprintf(stderr, "PASS: 0 check(s) failed\n");
	return failures ? 1 : 0;
}
