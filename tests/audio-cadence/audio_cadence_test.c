/* obs#57 (release gate) / obs#56 (dstack#303): OBS's audio thread feeds dante-input's TX through the
 * raw audio callback; a stall of 10-25 ms there makes dlink's TX player read drift and drop out. The
 * fork's ASRC follows a directly stamped source (STAMP basis), so dante-input's play-time stamps
 * (dstack#295: a block's play time, with dlink's clock-pair jitter and the media clock's drift) run
 * through it. Real libobs: a source pushes 128-frame blocks at 48 kHz from its own thread with direct
 * stamps, and a raw audio callback on mix 0 times every audio tick, 10 s each:
 *
 *   - smooth stamps;
 *   - a +-1.5 ms preempted-pair jitter every 2000 blocks;
 *   - the jitter plus 6 ppm drift (stream-snv's media clock vs CLOCK_MONOTONIC).
 *
 * Each must keep every tick-to-tick gap under 30 ms and the mean within 0.05 ms of 1024 frames
 * (21.333 ms).
 *
 *   audio-cadence-test
 *
 * Exit 0 = every check held. */

#include <obs.h>
#include <util/base.h>
#include <util/platform.h>
#include <util/threading.h>

#include <stdarg.h>
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
	return "cadence pusher";
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

enum stamps { SMOOTH, JITTER, JITTER_DRIFT };

struct run {
	obs_source_t *source;
	enum stamps stamps;
	volatile bool stop;
};

static void *push_loop(void *param)
{
	struct run *r = param;
	float plane[128];
	for (int i = 0; i < 128; i++)
		plane[i] = (float)(i % 32) / 64.0f;
	const uint64_t t0 = os_gettime_ns() + 20000000;
	for (uint64_t k = 0; !r->stop; k++) {
		const uint64_t due = t0 + k * 128ULL * 1000000000ULL / 48000ULL;
		if (due > os_gettime_ns())
			os_sleepto_ns(due);
		int64_t ts = (int64_t)due;
		if (r->stamps != SMOOTH && k % 2000 == 1000)
			ts += 1500000;
		if (r->stamps != SMOOTH && k % 2000 == 1001)
			ts -= 1500000;
		if (r->stamps == JITTER_DRIFT)
			ts += (int64_t)((double)(due - t0) * 6e-6);
		struct obs_source_audio a = {0};
		a.data[0] = (const uint8_t *)plane;
		a.data[1] = (const uint8_t *)plane;
		a.frames = 128;
		a.speakers = SPEAKERS_STEREO;
		a.format = AUDIO_FORMAT_FLOAT_PLANAR;
		a.samples_per_sec = 48000;
		a.timestamp = (uint64_t)ts;
		obs_source_output_audio(r->source, &a);
	}
	return NULL;
}

/* the raw audio callback's tick gaps, after the first 50 ticks (the mix settling) */
static uint64_t last_ns, ticks, max_gap_ns, gap_sum_ns, counted;

static void on_tick(void *param, size_t mix_idx, struct audio_data *data)
{
	(void)param;
	(void)mix_idx;
	(void)data;
	const uint64_t now = os_gettime_ns();
	if (last_ns && ++ticks > 50) {
		const uint64_t gap = now - last_ns;
		if (gap > max_gap_ns)
			max_gap_ns = gap;
		gap_sum_ns += gap;
		counted++;
	}
	last_ns = now;
}

static void run_case(enum stamps stamps, const char *name)
{
	last_ns = ticks = max_gap_ns = gap_sum_ns = counted = 0;
	struct run r = {0};
	r.stamps = stamps;
	r.source = obs_source_create("cadence_pusher", name, NULL, NULL);
	obs_set_output_source(0, r.source);
	struct audio_convert_info conv = {0};
	conv.samples_per_sec = 48000;
	conv.format = AUDIO_FORMAT_FLOAT_PLANAR;
	conv.speakers = SPEAKERS_STEREO;
	obs_add_raw_audio_callback(0, &conv, on_tick, NULL);
	pthread_t thread;
	pthread_create(&thread, NULL, push_loop, &r);
	os_sleep_ms(10000);
	r.stop = true;
	pthread_join(thread, NULL);
	obs_remove_raw_audio_callback(0, on_tick, NULL);
	obs_set_output_source(0, NULL);
	obs_source_release(r.source);

	const double mean_ms = counted ? (double)gap_sum_ns / 1e6 / (double)counted : 0.0;
	fprintf(stderr, "%s: %llu ticks, mean %.3f ms, max gap %.3f ms\n", name, (unsigned long long)counted, mean_ms,
		(double)max_gap_ns / 1e6);
	char what[160];
	snprintf(what, sizeof(what), "%s: ticks came (%llu)", name, (unsigned long long)counted);
	expect(counted > 300, what);
	snprintf(what, sizeof(what), "%s: every tick gap under 30 ms", name);
	expect(max_gap_ns < 30000000, what);
	snprintf(what, sizeof(what), "%s: the mean tick is 1024 frames at 48 kHz (21.333 ms +- 0.05)", name);
	expect(mean_ms > 21.283 && mean_ms < 21.383, what);
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
	info.id = "cadence_pusher";
	info.type = OBS_SOURCE_TYPE_INPUT;
	info.output_flags = OBS_SOURCE_AUDIO;
	info.get_name = pusher_name;
	info.create = pusher_create;
	info.destroy = pusher_destroy;
	obs_register_source(&info);

	run_case(SMOOTH, "smooth stamps");
	run_case(JITTER, "pair-jittered stamps");
	run_case(JITTER_DRIFT, "pair-jittered stamps + 6 ppm drift");

	obs_shutdown();
	if (failures)
		fprintf(stderr, "%d check(s) failed\n", failures);
	else
		fprintf(stderr, "PASS: 0 check(s) failed\n");
	return failures ? 1 : 0;
}
