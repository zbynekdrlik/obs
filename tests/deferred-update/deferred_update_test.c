/* obs#30: every strih OBS start stalled the graphics thread ~130 ms in one tick_sources: the first
 * ticks of 15 FreeType2 text sources, each running its deferred update (font, glyph cache, a 4 MB
 * texture; ~8.7 ms) there, since a video source's obs_source_update is deferred to its next video
 * tick. The program lagged 6-7 frames and the JPEG XS output dropped 2-3. libobs now gives deferred
 * updates a per-tick_sources budget: once spent, a source with an update still pending skips this
 * frame's tick (update and all) and runs it on a later frame. The test runs real libobs with 15 video
 * sources whose create defers an update of 8 ms, and checks:
 *
 *   - no gap between two tick_sources (a tick callback's calls) reaches 100 ms (it was 121 ms; with
 *     the budget about a frame + 8 ms);
 *   - every source's update ran;
 *   - no source's video_tick ran before its first update (OBS's order kept);
 *   - review: a video source with no update callback that gets obs_source_update keeps ticking, and
 *     a running source whose update is re-armed every frame keeps ticking while new sources wait.
 *
 *   deferred-update-test <libobs data dir>   (the SDK's share/obs/libobs/, which holds the effects)
 *
 * Exit 0 = every check held. */

#include <obs.h>
#include <util/base.h>
#include <util/platform.h>
#include <util/threading.h>

#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>

#define SOURCES 15
#define UPDATE_MS 8

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

/* Only the graphics thread touches these (updates, ticks and the tick callback all run there). */
static long updates;
static long ticks_before_update;

struct slow {
	bool updated;
};

static const char *slow_name(void *type_data)
{
	(void)type_data;
	return "slow update";
}

static void *slow_create(obs_data_t *settings, obs_source_t *source)
{
	(void)settings;
	struct slow *s = bzalloc(sizeof(*s));
	/* as text-freetype2 does: a video source's update is deferred to its next video tick */
	obs_source_update(source, NULL);
	return s;
}

static void slow_destroy(void *data)
{
	bfree(data);
}

static void slow_update(void *data, obs_data_t *settings)
{
	(void)settings;
	struct slow *s = data;
	os_sleep_ms(UPDATE_MS);
	s->updated = true;
	os_atomic_inc_long(&updates);
}

static void slow_tick(void *data, float seconds)
{
	(void)seconds;
	struct slow *s = data;
	if (!s->updated)
		os_atomic_inc_long(&ticks_before_update);
}

static uint32_t slow_size(void *data)
{
	(void)data;
	return 16;
}

static void slow_render(void *data, gs_effect_t *effect)
{
	(void)data;
	(void)effect;
}

/* the review's sources: ticks counted on the graphics thread */
static long plain_ticks, running_ticks;

static const char *plain_name(void *type_data)
{
	(void)type_data;
	return "no update";
}

static void plain_tick(void *data, float seconds)
{
	(void)data;
	(void)seconds;
	os_atomic_inc_long(&plain_ticks);
}

static void running_tick(void *data, float seconds)
{
	(void)data;
	(void)seconds;
	os_atomic_inc_long(&running_ticks);
}

static void *running_create(obs_data_t *settings, obs_source_t *source)
{
	(void)settings;
	(void)source;
	return bzalloc(1);
}

static void running_update(void *data, obs_data_t *settings)
{
	(void)data;
	(void)settings;
	os_sleep_ms(2);
}

static uint64_t last_tick_ns, max_gap_ns;

static void on_tick(void *param, float seconds)
{
	(void)param;
	(void)seconds;
	const uint64_t now = os_gettime_ns();
	if (last_tick_ns && now - last_tick_ns > max_gap_ns)
		max_gap_ns = now - last_tick_ns;
	last_tick_ns = now;
}

int main(int argc, char **argv)
{
	if (argc != 2) {
		fprintf(stderr, "usage: %s <libobs data dir>\n", argv[0]);
		return 2;
	}
	base_set_log_handler(log_line, NULL);
	if (!obs_startup("en-US", NULL, NULL)) {
		fprintf(stderr, "FAIL: obs_startup\n");
		return 1;
	}
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
	int reset = obs_reset_video(&ovi);
	expect(reset == OBS_VIDEO_SUCCESS, "obs_reset_video 64x64 NV12 on libobs-opengl");
	if (reset != OBS_VIDEO_SUCCESS) {
		obs_shutdown();
		return 1;
	}

	struct obs_source_info info = {0};
	info.id = "test_slow_update";
	info.type = OBS_SOURCE_TYPE_INPUT;
	info.output_flags = OBS_SOURCE_VIDEO;
	info.get_name = slow_name;
	info.create = slow_create;
	info.destroy = slow_destroy;
	info.update = slow_update;
	info.video_tick = slow_tick;
	info.get_width = slow_size;
	info.get_height = slow_size;
	info.video_render = slow_render;
	obs_register_source(&info);

	/* a video source with data but no update callback */
	struct obs_source_info plain = info;
	plain.id = "test_no_update";
	plain.get_name = plain_name;
	plain.create = running_create;
	plain.update = NULL;
	plain.video_tick = plain_tick;
	obs_register_source(&plain);
	/* a running source whose update is re-armed again and again */
	struct obs_source_info running = info;
	running.id = "test_running";
	running.get_name = plain_name;
	running.create = running_create;
	running.update = running_update;
	running.video_tick = running_tick;
	obs_register_source(&running);

	obs_source_t *no_update = obs_source_create("test_no_update", "no update", NULL, NULL);
	obs_source_t *busy = obs_source_create("test_running", "running", NULL, NULL);
	obs_source_update(busy, NULL);

	obs_add_tick_callback(on_tick, NULL);
	os_sleep_ms(500); /* the tick gaps settle; the running source has its first update */
	max_gap_ns = 0;

	/* An OBS start's scene load: all sources at once, before their first tick. */
	obs_source_t *sources[SOURCES];
	obs_enter_graphics();
	for (int i = 0; i < SOURCES; i++) {
		char name[32];
		snprintf(name, sizeof(name), "slow %d", i);
		sources[i] = obs_source_create("test_slow_update", name, NULL, NULL);
	}
	obs_source_update(no_update, NULL);
	obs_leave_graphics();
	const long plain_before = os_atomic_load_long(&plain_ticks);
	const long running_before = os_atomic_load_long(&running_ticks);
	for (int i = 0; i < 40; i++) {
		obs_source_update(no_update, NULL);
		obs_source_update(busy, NULL);
		os_sleep_ms(50);
	}
	obs_remove_tick_callback(on_tick, NULL);
	const long plain_n = os_atomic_load_long(&plain_ticks) - plain_before;
	const long running_n = os_atomic_load_long(&running_ticks) - running_before;
	fprintf(stderr, "in 2 s: the source with no update ticked %ld times, the running re-armed one %ld\n", plain_n,
		running_n);
	/* every frame, the 15 new sources' spread frames included (a held tick there would fall behind) */
	expect(running_n > 40 && plain_n >= running_n - 2,
	       "a video source with no update callback ticks every frame (its update is never held)");
	expect(running_n > 40, "a running source's re-armed updates never hold its ticks");

	fprintf(stderr, "updates %ld, ticks before an update %ld, max tick_sources gap %.1f ms\n",
		os_atomic_load_long(&updates), os_atomic_load_long(&ticks_before_update), (double)max_gap_ns / 1e6);
	expect(os_atomic_load_long(&updates) == SOURCES, "every source's deferred update ran");
	expect(max_gap_ns < 100000000, "no tick_sources gap reaches 100 ms (15 updates of 8 ms spread over frames)");
	expect(os_atomic_load_long(&ticks_before_update) == 0, "no source's video_tick ran before its first update");

	for (int i = 0; i < SOURCES; i++)
		obs_source_release(sources[i]);
	obs_source_release(no_update);
	obs_source_release(busy);
	obs_shutdown();
	if (failures)
		fprintf(stderr, "%d check(s) failed\n", failures);
	else
		fprintf(stderr, "PASS: 0 check(s) failed\n");
	return failures ? 1 : 0;
}
