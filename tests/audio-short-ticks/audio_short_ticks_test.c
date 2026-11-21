/* obs#94 (camera-box#1527): libobs counts, per source, the audio ticks whose render found the source's
 * buffer short of the tick (process_audio_source_tick: audio_pending, the source sits out that tick's
 * mix, no log). stream-snv runs FIXED 0 ms audio buffering, where a source that falls short of a tick
 * is silently left out; dante-input puts this count on its status line (late_to_mix). Real libobs:
 * a source pushes 128-frame blocks at 48 kHz, each 20 ms before its stamp (the soak's tone lead),
 * from its own thread:
 *
 *   - steady pushes: no short tick once the mix runs (counted from 3 s in to 8 s; the first tick
 *     after a source's first audio can count one, before the mix has settled on it);
 *   - the pusher stops for 150 ms and then catches up: the source's buffer runs dry, and at least one
 *     tick counts it short (so the count is shown able to rise);
 *   - a source on the mix that never pushed audio counts 0 (no audio_ts: never short, only silent).
 *
 *   audio-short-ticks-test
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
#include <string.h>

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
#define LEAD_NS 20000000ULL

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

/* One case of 8 s: the pusher's short ticks at 3 s and at 8 s, and the silent source's at 8 s. */
static bool run_case(const char *name, uint64_t stall_at, uint64_t *at_3s, uint64_t *at_8s, uint64_t *silent)
{
	struct run r = {0};
	r.stall_at = stall_at;
	atomic_init(&r.stop, false);
	r.source = obs_source_create("short_tick_pusher", name, NULL, NULL);
	obs_source_t *quiet = obs_source_create("short_tick_pusher", "never pushes", NULL, NULL);
	if (!r.source || !quiet) {
		expect(false, "the sources are created");
		obs_source_release(r.source);
		obs_source_release(quiet);
		return false;
	}
	obs_set_output_source(0, r.source);
	obs_set_output_source(1, quiet);
	pthread_t thread;
	const bool started = pthread_create(&thread, NULL, push_loop, &r) == 0;
	if (started) {
		os_sleep_ms(3000);
		*at_3s = obs_source_get_audio_short_ticks(r.source);
		os_sleep_ms(5000);
		*at_8s = obs_source_get_audio_short_ticks(r.source);
		*silent = obs_source_get_audio_short_ticks(quiet);
		atomic_store(&r.stop, true);
		pthread_join(thread, NULL);
	}
	obs_set_output_source(0, NULL);
	obs_set_output_source(1, NULL);
	obs_source_release(r.source);
	obs_source_release(quiet);
	if (!started) {
		expect(false, "the pusher thread starts");
		return false;
	}
	fprintf(stderr, "%s: short ticks %llu at 3 s, %llu at 8 s; the silent source %llu\n", name,
		(unsigned long long)*at_3s, (unsigned long long)*at_8s, (unsigned long long)*silent);
	return true;
}

int main(void)
{
	base_set_log_handler(log_line, NULL);
	if (!obs_startup("en-US", NULL, NULL)) {
		fprintf(stderr, "FAIL: obs_startup\n");
		return 1;
	}
	struct obs_audio_info ai = {0};
	ai.samples_per_sec = 48000;
	ai.speakers = SPEAKERS_STEREO;
	expect(obs_reset_audio(&ai), "obs_reset_audio 48 kHz stereo");

	struct obs_source_info info = {0};
	info.id = "short_tick_pusher";
	info.type = OBS_SOURCE_TYPE_INPUT;
	info.output_flags = OBS_SOURCE_AUDIO;
	info.get_name = pusher_name;
	info.create = pusher_create;
	info.destroy = pusher_destroy;
	obs_register_source(&info);

	expect(obs_source_get_audio_short_ticks(NULL) == 0, "a NULL source reads 0");

	uint64_t at_3s = 0, at_8s = 0, silent = 0;
	if (run_case("steady pushes", 0, &at_3s, &at_8s, &silent)) {
		expect(at_8s == at_3s, "steady pushes: no short tick from 3 s to 8 s");
		expect(silent == 0, "steady case: a source that never pushed counts no short tick");
	}
	/* block 1500 = 4 s in: inside the 3-8 s window */
	if (run_case("a 150 ms push stall", 1500, &at_3s, &at_8s, &silent)) {
		expect(at_8s > at_3s, "a 150 ms push stall: the dry buffer counts short ticks");
		expect(silent == 0, "stall case: a source that never pushed counts no short tick");
	}

	obs_shutdown();
	if (failures)
		fprintf(stderr, "%d check(s) failed\n", failures);
	else
		fprintf(stderr, "PASS: 0 check(s) failed\n");
	return failures ? 1 : 0;
}
