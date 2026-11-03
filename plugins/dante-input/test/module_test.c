/* The dante_input module through real libobs (obs#1): load it as OBS does and check the contract
 * from the outside.
 *
 *   module_test <dante-input.so> absent     HOME has no .camera-box/dante-input.toml:
 *                                           no source kind, no device.
 *   module_test <dante-input.so> bad-iface  the config names an interface the box lacks: the kind
 *                                           is registered, the device is not, `dante-input error:`.
 *   module_test <dante-input.so> running    the config names a real interface: the device runs, a
 *                                           source is created and destroyed, unload stops it.
 *
 * The caller writes HOME/.camera-box/dante-input.toml for the last two. Exit 0 = every check held. */

#include <obs.h>
#include <util/base.h>
#include <util/dstr.h>
#include <util/platform.h>
#include <util/threading.h>

#include <stdarg.h>
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
		fprintf(stderr, "usage: %s <dante-input.so> absent|bad-iface|running\n", argv[0]);
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
	} else {
		fprintf(stderr, "unknown mode %s\n", mode);
		return 2;
	}

	obs_shutdown();
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
