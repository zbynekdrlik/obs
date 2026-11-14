/* obs#30: the tick audit names a slow video tick.
 *
 * On strih-snv every OBS start lost 2-3 JPEG XS frames and 6-7 program frames in its first 5 s. The
 * exit profiles of three starts showed one tick_sources of 70-117 ms per start (its p99 0.4-0.7 ms):
 * a source's or a tick callback's video tick on the graphics thread, which OBS's profiler does not
 * split. libobs's tick_sources now times each one, and one over 20 ms logs a `tick-audit:` line. The
 * test runs real libobs and libobs-opengl and checks:
 *
 *   - a source whose tick takes 60 ms is named, with its id and the time;
 *   - a fast source is not named;
 *   - a tick callback that takes 45 ms is named by its symbol;
 *   - at most 64 slow ticks are logged, then one line says further ones are not.
 *
 *   tick-audit-test <libobs data dir>   (the SDK's share/obs/libobs/, which holds the effects)
 *
 * Exit 0 = every check held. */

#include <obs.h>
#include <util/base.h>
#include <util/platform.h>
#include <util/threading.h>

#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int failures;

static void expect(bool ok, const char *what)
{
	fprintf(stderr, "%s: %s\n", ok ? "OK" : "FAIL", what);
	if (!ok)
		failures++;
}

/* ---- the log, kept: the audit lines are what the test reads ---- */

#define MAX_LINES 4096
static pthread_mutex_t log_mutex = PTHREAD_MUTEX_INITIALIZER;
static char *lines[MAX_LINES];
static size_t line_count;

static void log_line(int level, const char *fmt, va_list args, void *param)
{
	(void)param;
	char line[4096];
	vsnprintf(line, sizeof(line), fmt, args);
	fprintf(stderr, "[%d] %s\n", level, line);
	pthread_mutex_lock(&log_mutex);
	if (line_count < MAX_LINES)
		lines[line_count++] = strdup(line);
	pthread_mutex_unlock(&log_mutex);
}

/* the number of kept lines holding every one of the (NULL-ended) parts */
static size_t count_lines(const char *first, ...)
{
	size_t n = 0;
	pthread_mutex_lock(&log_mutex);
	for (size_t i = 0; i < line_count; i++) {
		bool all = true;
		va_list parts;
		va_start(parts, first);
		for (const char *p = first; p; p = va_arg(parts, const char *)) {
			if (!strstr(lines[i], p)) {
				all = false;
				break;
			}
		}
		va_end(parts);
		if (all)
			n++;
	}
	pthread_mutex_unlock(&log_mutex);
	return n;
}

/* ---- sources whose video tick takes a set time ---- */

struct ticker {
	uint32_t slow_ms;   /* each slow tick's sleep */
	uint32_t slow_left; /* slow ticks left; UINT32_MAX = every tick */
};

static const char *ticker_name(void *type_data)
{
	(void)type_data;
	return "test ticker";
}

static void *ticker_create(obs_data_t *settings, obs_source_t *source)
{
	(void)source;
	struct ticker *t = bzalloc(sizeof(*t));
	t->slow_ms = (uint32_t)obs_data_get_int(settings, "slow_ms");
	t->slow_left = (uint32_t)obs_data_get_int(settings, "slow_ticks");
	return t;
}

static void ticker_destroy(void *data)
{
	bfree(data);
}

static void ticker_tick(void *data, float seconds)
{
	(void)seconds;
	struct ticker *t = data;
	if (t->slow_left == 0)
		return;
	if (t->slow_left != UINT32_MAX)
		t->slow_left--;
	os_sleep_ms(t->slow_ms);
}

static uint32_t ticker_size(void *data)
{
	(void)data;
	return 16;
}

static void ticker_render(void *data, gs_effect_t *effect)
{
	(void)data;
	(void)effect;
}

static obs_source_t *ticker(const char *id, const char *name, int64_t slow_ms, int64_t slow_ticks)
{
	obs_data_t *s = obs_data_create();
	obs_data_set_int(s, "slow_ms", slow_ms);
	obs_data_set_int(s, "slow_ticks", slow_ticks);
	obs_source_t *source = obs_source_create(id, name, s, NULL);
	obs_data_release(s);
	return source;
}

static void register_ticker(const char *id)
{
	struct obs_source_info info = {0};
	info.id = id;
	info.type = OBS_SOURCE_TYPE_INPUT;
	info.output_flags = OBS_SOURCE_VIDEO;
	info.get_name = ticker_name;
	info.create = ticker_create;
	info.destroy = ticker_destroy;
	info.get_width = ticker_size;
	info.get_height = ticker_size;
	info.video_tick = ticker_tick;
	info.video_render = ticker_render;
	obs_register_source(&info);
}

/* ---- a tick callback slow once ---- */

static volatile long callback_slow_left = 1;

void tick_audit_slow_callback(void *param, float seconds)
{
	(void)param;
	(void)seconds;
	if (os_atomic_dec_long(&callback_slow_left) == 0)
		os_sleep_ms(45);
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
	/* The SDK is not where libobs was configured to live: name its data dir (the effects). */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
	obs_add_data_path(argv[1]);
#pragma GCC diagnostic pop

	struct obs_video_info ovi = {0};
	ovi.graphics_module = "libobs-opengl";
	ovi.fps_num = 30;
	ovi.fps_den = 1;
	ovi.base_width = 64;
	ovi.base_height = 64;
	ovi.output_width = 64;
	ovi.output_height = 64;
	ovi.output_format = VIDEO_FORMAT_NV12;
	ovi.adapter = 0;
	ovi.gpu_conversion = true;
	ovi.colorspace = VIDEO_CS_709;
	ovi.range = VIDEO_RANGE_PARTIAL;
	ovi.scale_type = OBS_SCALE_BICUBIC;
	int reset = obs_reset_video(&ovi);
	expect(reset == OBS_VIDEO_SUCCESS, "obs_reset_video 64x64 NV12 on libobs-opengl");
	if (reset != OBS_VIDEO_SUCCESS) {
		obs_shutdown();
		fprintf(stderr, "%d check(s) failed\n", failures);
		return 1;
	}

	register_ticker("test_slow_once");
	register_ticker("test_fast");
	register_ticker("test_slow_always");

	/* A source slow on its first 2 ticks (60 ms, as a first tick at an OBS start), a fast one and
	 * a tick callback slow once (45 ms). */
	obs_source_t *slow = ticker("test_slow_once", "slow one", 60, 2);
	obs_source_t *fast = ticker("test_fast", "fast one", 0, 0);
	obs_add_tick_callback(tick_audit_slow_callback, NULL);
	os_sleep_ms(1500);

	expect(count_lines("tick-audit:", "source 'slow one'", "(test_slow_once)", "video tick took 6", NULL) == 2,
	       "the slow source's 2 slow ticks are named, with its id and ~60 ms");
	expect(count_lines("tick-audit:", "'fast one'", NULL) == 0, "the fast source is not named");
	expect(count_lines("tick-audit:", "tick callback", "tick_audit_slow_callback", "took 4", NULL) == 1,
	       "the slow tick callback is named by its symbol, ~45 ms");

	/* The cap: a source slow on every tick fills the 64 lines, then one line says so. */
	obs_source_t *always = ticker("test_slow_always", "slow always", 21, UINT32_MAX);
	os_sleep_ms(4000);
	size_t audit_lines = count_lines("tick-audit:", "took", NULL);
	fprintf(stderr, "tick-audit lines naming a tick: %zu\n", audit_lines);
	expect(audit_lines == 64, "at most 64 slow ticks are logged");
	expect(count_lines("tick-audit:", "64 slow video ticks logged; further ones are not logged", NULL) == 1,
	       "one line says further slow ticks are not logged");

	obs_remove_tick_callback(tick_audit_slow_callback, NULL);
	obs_source_release(always);
	obs_source_release(fast);
	obs_source_release(slow);
	obs_shutdown();

	if (failures)
		fprintf(stderr, "%d check(s) failed\n", failures);
	else
		fprintf(stderr, "PASS: 0 check(s) failed\n");
	return failures ? 1 : 0;
}
