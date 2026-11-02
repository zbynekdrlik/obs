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

#include <stdarg.h>
#include <stdio.h>
#include <string.h>

static struct dstr logged;

static void capture(int level, const char *fmt, va_list args, void *param)
{
	(void)param;
	char line[4096];
	vsnprintf(line, sizeof(line), fmt, args);
	fprintf(stderr, "[%d] %s\n", level, line);
	dstr_cat(&logged, line);
	dstr_cat(&logged, "\n");
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
	return logged.array && strstr(logged.array, s) != NULL;
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
		expect(logged_has(", 2 channels at 96000 Hz)"), "the source runs at the device rate, 96 kHz");
		/* Let the pump run blocks into the source. */
		os_sleep_ms(2000);
		obs_source_release(src);
		expect(logged_has("dante-input source: destroyed (0 source(s) left)"), "the source is destroyed");
	} else {
		fprintf(stderr, "unknown mode %s\n", mode);
		return 2;
	}

	obs_shutdown();
	if (strcmp(mode, "running") == 0) {
		expect(logged_has("dante-input stop: device stopped (Done)"), "unload stops the device cleanly");
		expect(!logged_has("did not end within the stop wait"), "no pump left running");
	}
	if (strcmp(mode, "absent") != 0)
		expect(logged_has("dante-input stop: module unloaded"), "the module unloads");

	fprintf(stderr, "%s: %d failure(s)\n", mode, failures);
	dstr_free(&logged);
	return failures ? 1 : 0;
}
