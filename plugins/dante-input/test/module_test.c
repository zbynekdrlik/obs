/* The dante_input module through real libobs (obs#1): load it as OBS does and check the contract
 * from the outside.
 *
 *   module_test <dante-input.so> absent     HOME has no .camera-box/dante-input.toml:
 *                                           no source kind, no device.
 *   module_test <dante-input.so> bad-iface  the config names an interface the box lacks: the kind
 *                                           is registered, the device is not, `dante-input error:`.
 *   module_test <dante-input.so> running    the config names a real interface: the device runs, a
 *                                           source is created and destroyed, unload stops it.
 *   module_test <dante-input.so> running-tx the config also has `tx` (obs#41): OBS's track 1 mix
 *                                           reaches the device's TX FIFO (the status line's tx_*
 *                                           tokens move), unload untaps it before the stop.
 *
 *   module_test <dante-input.so> soak       obs#57's release gate (dstack docs/ci-peer.md): the config
 *                                           has tx, test_clock_ppm and an RX subscribed to a running
 *                                           `dstack-dev peer` (DANTE_PEER_STATUS names its status
 *                                           JSON). OBS plays a 440 Hz tone on track 1 into the TX; a
 *                                           dante_input source (track 2 only) takes the peer's tone.
 *                                           After the settle, DANTE_SOAK_SECONDS (600 unless set) of
 *                                           real time: no TX underrun, re-centre or resync, no RX
 *                                           hole, loss or gap, no pump underrun, no play-time step,
 *                                           stamp jump or unfilled gap, no peer tone gap; the play
 *                                           time's deviation under 1 ms, tx_ppm and the peer's
 *                                           play_at_ppm within the skew + 2 ppm.
 *
 * The caller writes HOME/.camera-box/dante-input.toml for the last three. Exit 0 = every check held. */

#include <obs.h>
#include <util/base.h>
#include <util/dstr.h>
#include <util/platform.h>
#include <util/threading.h>

#include <math.h>
#include <stdarg.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>

/* libobs logs from its own threads too (audio, destroy), so the buffer is locked. */
static pthread_mutex_t logged_mutex = PTHREAD_MUTEX_INITIALIZER;
static struct dstr logged;

static void capture(int level, const char *fmt, va_list args, void *param)
{
	(void)param;
	char line[4096];
	vsnprintf(line, sizeof(line), fmt, args);
	fprintf(stderr, "[%d] %s\n", level, line);
	pthread_mutex_lock(&logged_mutex);
	dstr_cat(&logged, line);
	dstr_cat(&logged, "\n");
	pthread_mutex_unlock(&logged_mutex);
}

static int failures;

static void expect(bool ok, const char *what)
{
	fprintf(stderr, "%s: %s\n", ok ? "OK" : "FAIL", what);
	if (!ok)
		failures++;
}

static bool logged_has(const char *s)
{
	pthread_mutex_lock(&logged_mutex);
	bool found = logged.array && strstr(logged.array, s) != NULL;
	pthread_mutex_unlock(&logged_mutex);
	return found;
}

/* Whether a mapping of this process names `name`. /proc files report size 0, so they are read
 * line by line, not by a size-first reader. */
static bool mapped(const char *name)
{
	FILE *f = fopen("/proc/self/maps", "r");
	if (!f)
		return false;
	char line[4096];
	bool found = false;
	while (!found && fgets(line, sizeof(line), f))
		found = strstr(line, name) != NULL;
	fclose(f);
	return found;
}

/* The value of ` <token>=` in the last status line that has it; 0 when none has. */
static unsigned long long last_value(const char *token)
{
	unsigned long long v = 0;
	pthread_mutex_lock(&logged_mutex);
	const char *p = logged.array;
	while (p && (p = strstr(p, token)) != NULL) {
		p += strlen(token);
		v = strtoull(p, NULL, 10);
	}
	pthread_mutex_unlock(&logged_mutex);
	return v;
}

/* The value of ` <token>=` (a decimal, maybe negative) in the last status line that has it. */
static double last_double(const char *token)
{
	double v = 0.0;
	pthread_mutex_lock(&logged_mutex);
	const char *p = logged.array;
	while (p && (p = strstr(p, token)) != NULL) {
		p += strlen(token);
		v = strtod(p, NULL);
	}
	pthread_mutex_unlock(&logged_mutex);
	return v;
}

static bool logged_within(const char *s, int ms);

/* ---- obs#57's soak ---- */

/* The peer's own object of its status JSON (from `"peer":{` on: the file puts "device" first, and
 * the device's keys must not be read as the peer's), or NULL; the caller frees it. */
static char *peer_part(void)
{
	const char *path = getenv("DANTE_PEER_STATUS");
	char *text = path ? os_quick_read_utf8_file(path) : NULL;
	if (!text)
		return NULL;
	const char *peer = strstr(text, "\"peer\":{");
	char *part = peer ? bstrdup(peer) : NULL;
	bfree(text);
	return part;
}

/* The number after `"key":` in `json` (spaces allowed), or -1 when absent. */
static double json_number(const char *json, const char *key)
{
	char quoted[64];
	snprintf(quoted, sizeof(quoted), "\"%s\"", key);
	const char *p = json ? strstr(json, quoted) : NULL;
	if (!p)
		return -1.0;
	p = strchr(p + strlen(quoted), ':');
	return p ? strtod(p + 1, NULL) : -1.0;
}

/* The sum of every `"key":` number in `json` (each RX channel's), or -1 when none. */
static double json_sum(const char *json, const char *key)
{
	char quoted[64];
	snprintf(quoted, sizeof(quoted), "\"%s\"", key);
	double sum = -1.0;
	for (const char *p = json ? strstr(json, quoted) : NULL; p; p = strstr(p + 1, quoted)) {
		const char *colon = strchr(p + strlen(quoted), ':');
		if (colon)
			sum = (sum < 0 ? 0.0 : sum) + strtod(colon + 1, NULL);
	}
	return sum;
}

/* Every peer RX channel locked on a tone (one "locked": true and no "locked": false). */
static bool all_locked(const char *json)
{
	return json && (strstr(json, "\"locked\": true") || strstr(json, "\"locked\":true")) &&
	       !strstr(json, "\"locked\": false") && !strstr(json, "\"locked\":false");
}

static bool peer_locked(void)
{
	char *json = peer_part();
	bool locked = all_locked(json);
	bfree(json);
	return locked;
}

/* A 440 Hz tone at -12 dBFS, OBS's track 1 into dante-input's TX: steady for the peer's check. */
static const char *tone_name(void *type_data)
{
	(void)type_data;
	return "soak tone";
}

static void *tone_create(obs_data_t *settings, obs_source_t *source)
{
	(void)settings;
	(void)source;
	return bzalloc(1);
}

static void tone_destroy(void *data)
{
	bfree(data);
}

static volatile bool tone_stop;
static obs_source_t *tone_source;

static void *tone_loop(void *param)
{
	(void)param;
	float plane[480];
	const uint64_t t0 = os_gettime_ns();
	uint64_t frame = 0;
	for (uint64_t k = 0; !tone_stop; k++) {
		const uint64_t due = t0 + k * 480ULL * 1000000000ULL / 48000ULL;
		if (due > os_gettime_ns())
			os_sleepto_ns(due);
		for (int i = 0; i < 480; i++, frame++)
			plane[i] = 0.25f * (float)sin(2.0 * M_PI * 440.0 * (double)frame / 48000.0);
		struct obs_source_audio a = {0};
		a.data[0] = (const uint8_t *)plane;
		a.data[1] = (const uint8_t *)plane;
		a.frames = 480;
		a.speakers = SPEAKERS_STEREO;
		a.format = AUDIO_FORMAT_FLOAT_PLANAR;
		a.samples_per_sec = 48000;
		a.timestamp = due;
		obs_source_output_audio(tone_source, &a);
	}
	return NULL;
}

/* The status line's counters that must not rise during the soak. */
static const char *const soak_counters[] = {
	" tx_underruns=", " tx_recentres=", " tx_resyncs=",  " rx_holes=",    " rx_loss=",
	" rx_gaps=",      " underruns=",    " play_at_steps=", " stamp_jumps=", " gaps_unfilled=",
};
#define SOAK_COUNTERS (sizeof(soak_counters) / sizeof(soak_counters[0]))

static void soak(double skew_ppm)
{
	/* OBS activates its output channels' sources from the video thread's tick: with audio only,
	 * neither the tone nor the dante source would be mixed (the TX would carry silence). */
	const char *data = getenv("DANTE_LIBOBS_DATA");
	expect(data != NULL, "DANTE_LIBOBS_DATA names libobs's data dir (the effects)");
	if (!data)
		return;
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
	obs_add_data_path(data);
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
	expect(obs_reset_video(&ovi) == OBS_VIDEO_SUCCESS, "obs_reset_video 64x64 (activates the sources)");

	struct obs_source_info tone = {0};
	tone.id = "soak_tone";
	tone.type = OBS_SOURCE_TYPE_INPUT;
	tone.output_flags = OBS_SOURCE_AUDIO;
	tone.get_name = tone_name;
	tone.create = tone_create;
	tone.destroy = tone_destroy;
	obs_register_source(&tone);
	tone_source = obs_source_create("soak_tone", "soak tone", NULL, NULL);
	obs_source_set_audio_mixers(tone_source, 1);
	obs_set_output_source(1, tone_source);
	pthread_t tone_thread;
	expect(pthread_create(&tone_thread, NULL, tone_loop, NULL) == 0, "the tone thread starts");

	/* The peer's tone through dante-input into OBS, on track 2 only (the TX carries one tone). */
	obs_source_t *dante = obs_source_create("dante_input", "Dante soak", NULL, NULL);
	expect(dante != NULL, "a dante_input source is created");
	obs_source_set_audio_mixers(dante, 2);
	obs_set_output_source(2, dante);

	expect(logged_has("(device running)"), "the device runs");
	expect(logged_within(" tx_priming=0", 20000), "the TX player primed on OBS's tone");
	bool locked = false;
	for (int waited = 0; waited < 60000 && !locked; waited += 500) {
		locked = peer_locked() && last_value(" rx_packets=") > 0;
		if (!locked)
			os_sleep_ms(500);
	}
	expect(locked, "the peer's RX channels all locked on dante-input's TX tone and dante-input receives the peer");
	os_sleep_ms(15000); /* the settle: the status line every 10 s from 30 s on, the ASRCs lock */

	unsigned long long before[SOAK_COUNTERS];
	for (size_t i = 0; i < SOAK_COUNTERS; i++)
		before[i] = last_value(soak_counters[i]);
	char *json = peer_part();
	const double gaps_before = json_sum(json, "tone_gaps");
	bfree(json);
	const char *window_env = getenv("DANTE_SOAK_SECONDS");
	const int window = window_env ? atoi(window_env) : 600;
	fprintf(stderr, "soak: %d s from here (skew %.1f ppm)\n", window, skew_ppm);
	os_sleep_ms((uint32_t)window * 1000 + 11000); /* + one 10 s status line */

	for (size_t i = 0; i < SOAK_COUNTERS; i++) {
		const unsigned long long after = last_value(soak_counters[i]);
		char what[160];
		snprintf(what, sizeof(what), "soak: %s did not rise (%llu then %llu)", soak_counters[i] + 1, before[i],
			 after);
		expect(after == before[i], what);
	}
	const unsigned long long dev = last_value(" play_at_max_dev_us=");
	char what[160];
	snprintf(what, sizeof(what), "soak: play_at_max_dev_us %llu under 1000", dev);
	expect(dev < 1000, what);
	const double tx_ppm = last_double(" tx_ppm=");
	snprintf(what, sizeof(what), "soak: tx_ppm %.1f settled within the skew + 2 (|%.1f| <= %.1f)", tx_ppm, tx_ppm,
		 fabs(skew_ppm) + 2.0);
	expect(fabs(tx_ppm) <= fabs(skew_ppm) + 2.0, what);
	json = peer_part();
	const double gaps_after = json_sum(json, "tone_gaps");
	const double peer_ppm = json_number(json, "play_at_ppm");
	const bool still_locked = all_locked(json);
	bfree(json);
	snprintf(what, sizeof(what), "soak: the peer's tone_gaps did not rise (%.0f then %.0f)", gaps_before,
		 gaps_after);
	expect(gaps_before >= 0 && gaps_after == gaps_before, what);
	expect(still_locked, "soak: the peer's RX still locked at the end");
	snprintf(what, sizeof(what), "soak: the peer's play_at_ppm %.2f within the skew %.1f ± 2", peer_ppm, skew_ppm);
	expect(fabs(peer_ppm - skew_ppm) <= 2.0, what);

	tone_stop = true;
	pthread_join(tone_thread, NULL);
	obs_set_output_source(1, NULL);
	obs_set_output_source(2, NULL);
	obs_source_release(tone_source);
	obs_source_release(dante);
}

/* `s` logged within `ms` (libobs destroys a released source on another thread). */
static bool logged_within(const char *s, int ms)
{
	for (int waited = 0; waited <= ms; waited += 50) {
		if (logged_has(s))
			return true;
		os_sleep_ms(50);
	}
	return false;
}

int main(int argc, char **argv)
{
	if (argc != 3) {
		fprintf(stderr, "usage: %s <dante-input.so> absent|bad-iface|running|running-tx|soak\n", argv[0]);
		return 2;
	}
	const char *so = argv[1];
	const char *mode = argv[2];
	base_set_log_handler(capture, NULL);

	if (!obs_startup("en-US", NULL, NULL)) {
		fprintf(stderr, "FAIL: obs_startup\n");
		return 1;
	}
	struct obs_audio_info ai = {.samples_per_sec = 48000, .speakers = SPEAKERS_STEREO};
	expect(obs_reset_audio(&ai), "obs_reset_audio 48 kHz stereo");

	obs_module_t *module = NULL;
	expect(obs_open_module(&module, so, NULL) == MODULE_SUCCESS, "obs_open_module");
	expect(module && obs_init_module(module), "obs_init_module");
	const char *kind = obs_source_get_display_name("dante_input");

	if (strcmp(mode, "absent") == 0) {
		expect(kind == NULL, "no dante_input kind without the config");
		expect(logged_has("dante-input start: no "), "the no-config line");
		expect(!logged_has("dante-input error:"), "no error line");
		expect(!logged_has("dante-input dlink:"), "no dlink line (no device)");
	} else if (strcmp(mode, "bad-iface") == 0) {
		expect(kind && strcmp(kind, "Dante input") == 0, "the dante_input kind is registered");
		expect(logged_has("dante-input error:"), "the error line");
		expect(logged_has("(device NOT running)"), "the device is not running");
	} else if (strcmp(mode, "running") == 0) {
		expect(kind && strcmp(kind, "Dante input") == 0, "the dante_input kind is registered");
		expect(logged_has("(device running)"), "the device runs");
		obs_source_t *src = obs_source_create("dante_input", "Dante test", NULL, NULL);
		expect(src != NULL, "a dante_input source is created");
		expect(logged_has("dante-input source: 'Dante test' created (1 source(s) now; device running"),
		       "the create line names the running device");
		expect(logged_has(", 2 channels at 48000 Hz)"), "OBS gets 48 kHz blocks from the 96 kHz network");
		expect(logged_has("device_rate=96000 converts=true"), "the device runs at the network's 96 kHz");
		/* The pump's blocks reach the source. */
		expect(logged_within("dante-input source: first block delivered (", 2000),
		       "a block reached the source within 2 s");
		expect(logged_has("channels at 48000 Hz, 1 source(s))"), "the block is 48 kHz, to the one source");
		os_sleep_ms(500);
		obs_source_release(src);
		expect(logged_within("dante-input source: destroyed (0 source(s) left)", 3000),
		       "the source is destroyed within 3 s");
	} else if (strcmp(mode, "running-tx") == 0) {
		expect(logged_has("(device running)"), "the device runs");
		expect(logged_has("1 RX channels, 2 TX channels") || logged_has("2 RX channels, 2 TX channels"),
		       "the device has its 2 TX channels");
		expect(logged_has("dante-input start: TX taps OBS track 1 (mix 0) at 48000 Hz"),
		       "the TX taps OBS's track 1 at OBS's 48 kHz");
		/* OBS's audio thread outputs the mix on its own; the FIFO primes at 40 ms and the
		 * player then plays it. The status line comes every second at the start. */
		expect(logged_within(" tx_track=1 ", 3000), "the status line carries the TX track");
		expect(logged_within(" tx_priming=0", 4000), "the TX player primed on OBS's mix");
		unsigned long long first = last_value(" tx_played=");
		os_sleep_ms(2500);
		unsigned long long later = last_value(" tx_played=");
		expect(later > first && first > 0, "tx_played grows while OBS runs");
		fprintf(stderr, "tx_played %llu then %llu\n", first, later);
		expect(last_value(" tx_overflows=") == 0 && logged_has(" tx_overflows="),
		       "no OBS frame was dropped at the FIFO (the last status line)");
	} else if (strcmp(mode, "soak") == 0) {
		const char *skew = getenv("DANTE_SOAK_SKEW_PPM");
		soak(skew ? atof(skew) : 6.0);
	} else {
		fprintf(stderr, "unknown mode %s\n", mode);
		return 2;
	}

	obs_shutdown();
	if (strcmp(mode, "running-tx") == 0) {
		expect(logged_has("dante-input stop: TX untapped OBS mix 0"), "unload untaps the TX");
		expect(logged_has("dante-input stop: device stopped (Done)"), "unload stops the device cleanly");
		pthread_mutex_lock(&logged_mutex);
		const char *untap = strstr(logged.array, "TX untapped");
		const char *stopped = strstr(logged.array, "dante-input stop: device stopped");
		pthread_mutex_unlock(&logged_mutex);
		expect(untap && stopped && untap < stopped, "the TX is untapped before the device stops");
	}
	if (strcmp(mode, "running") == 0) {
		/* RTLD_NODELETE held: the module stays mapped after OBS unloaded it. */
		expect(mapped("dante-input.so"), "the module stays mapped after unload (pinned)");
		expect(logged_has("held loaded for the process"), "the pin was logged");
		expect(logged_has("dante-input stop: device stopped (Done)"), "unload stops the device cleanly");
		expect(!logged_has("did not end within the stop wait"), "no pump left running");
	}
	if (strcmp(mode, "absent") != 0)
		expect(logged_has("dante-input stop: module unloaded"), "the module unloads");

	fprintf(stderr, "%s: %d failure(s)\n", mode, failures);
	dstr_free(&logged);
	return failures ? 1 : 0;
}
