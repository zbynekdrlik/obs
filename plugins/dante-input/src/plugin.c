/* The OBS `dante_input` source (obs#1, camera-box#1485): Dante audio from the box's dlink device.
 *
 * The device belongs to the process, not to a source: it starts at module load when
 * ~/.camera-box/dante-input.toml exists (camera-box's deploy writes it) and stops at unload, so its
 * Dante subscriptions stay up whatever the scenes do. Without the file there is no device and no
 * source kind at all: the box behaves as before. Every source of the kind outputs the device's RX
 * channels. */

#define _GNU_SOURCE
#include <dlfcn.h>

#include <obs-module.h>
#include <util/darray.h>
#include <util/dstr.h>
#include <util/platform.h>
#include <util/threading.h>
#include <util/util_uint64.h>

#include <stdlib.h>

#include "dante_input.h"

OBS_DECLARE_MODULE()

#define DANTE_INPUT_ID "dante_input"
/* The most sources one block goes to at once (a scene collection has one, maybe two). */
#define DANTE_INPUT_MAX_SOURCES 16
/* A block stamp further than this from the system clock is re-anchored on it. */
#define DANTE_INPUT_RESYNC_NS 20000000ULL

static struct dante_input *device;
static uint32_t device_channels;
/* Written once after the start; the pump thread reads it (0: not known yet, the block is skipped). */
static volatile long device_rate;
static pthread_mutex_t sources_mutex = PTHREAD_MUTEX_INITIALIZER;
static DARRAY(obs_source_t *) sources;
static bool registered;
/* The pump thread's own: the stamp the next block gets, and the re-anchors. */
static uint64_t next_ts;
static uint64_t resyncs;

static enum speaker_layout layout_of(uint32_t channels)
{
	switch (channels) {
	case 1:
		return SPEAKERS_MONO;
	case 2:
		return SPEAKERS_STEREO;
	case 3:
		return SPEAKERS_2POINT1;
	case 4:
		return SPEAKERS_4POINT0;
	case 5:
		return SPEAKERS_4POINT1;
	case 6:
		return SPEAKERS_5POINT1;
	case 8:
		return SPEAKERS_7POINT1;
	default:
		return SPEAKERS_UNKNOWN;
	}
}

static void on_log(int level, const char *msg)
{
	blog(level, "%s", msg);
}

/* Blocks are stamped by their sample count, so OBS sees the device's steady pace, not the
 * pump's wake jitter; a stamp that drifts past DANTE_INPUT_RESYNC_NS from the system clock (a gap,
 * a stall, clock drift) is re-anchored on it. */
static uint64_t stamp(uint32_t frames, uint32_t rate)
{
	uint64_t now = os_gettime_ns();
	uint64_t diff = now > next_ts ? now - next_ts : next_ts - now;
	if (next_ts == 0 || diff > DANTE_INPUT_RESYNC_NS) {
		if (next_ts != 0) {
			resyncs++;
			blog(LOG_INFO, "dante-input timing: re-anchored on the system clock (off by %llu us, %llu re-anchor(s))",
			     (unsigned long long)(diff / 1000), (unsigned long long)resyncs);
		}
		next_ts = now;
	}
	uint64_t ts = next_ts;
	next_ts += util_mul_div64(frames, 1000000000ULL, rate);
	return ts;
}

/* The device's pump thread, every block. */
static void on_audio(void *opaque, const float *const *planes, uint32_t channels, uint32_t frames)
{
	UNUSED_PARAMETER(opaque);
	uint32_t rate = (uint32_t)os_atomic_load_long(&device_rate);
	if (rate == 0)
		return;
	struct obs_source_audio audio = {0};
	for (uint32_t c = 0; c < channels && c < MAX_AV_PLANES; c++)
		audio.data[c] = (const uint8_t *)planes[c];
	audio.frames = frames;
	audio.speakers = layout_of(channels);
	audio.format = AUDIO_FORMAT_FLOAT_PLANAR;
	audio.samples_per_sec = rate;
	audio.timestamp = stamp(frames, rate);

	/* Strong refs under the lock, the output outside it: OBS's own locks are never taken while
	 * sources_mutex is held, so source create/destroy cannot deadlock against the pump. */
	obs_source_t *targets[DANTE_INPUT_MAX_SOURCES];
	size_t n = 0;
	pthread_mutex_lock(&sources_mutex);
	for (size_t i = 0; i < sources.num && n < DANTE_INPUT_MAX_SOURCES; i++) {
		obs_source_t *ref = obs_source_get_ref(sources.array[i]);
		if (ref)
			targets[n++] = ref;
	}
	pthread_mutex_unlock(&sources_mutex);
	for (size_t i = 0; i < n; i++) {
		obs_source_output_audio(targets[i], &audio);
		obs_source_release(targets[i]);
	}
}

static const char *dante_input_name(void *type_data)
{
	UNUSED_PARAMETER(type_data);
	return "Dante input";
}

static void *dante_input_create(obs_data_t *settings, obs_source_t *source)
{
	UNUSED_PARAMETER(settings);
	pthread_mutex_lock(&sources_mutex);
	da_push_back(sources, &source);
	size_t n = sources.num;
	pthread_mutex_unlock(&sources_mutex);
	blog(LOG_INFO, "dante-input source: '%s' created (%zu source(s) now; device %s, %u channels at %u Hz)",
	     obs_source_get_name(source), n, device ? "running" : "NOT running", device_channels,
	     (unsigned)os_atomic_load_long(&device_rate));
	return source;
}

static void dante_input_destroy(void *data)
{
	obs_source_t *source = data;
	pthread_mutex_lock(&sources_mutex);
	da_erase_item(sources, &source);
	size_t n = sources.num;
	pthread_mutex_unlock(&sources_mutex);
	blog(LOG_INFO, "dante-input source: destroyed (%zu source(s) left)", n);
}

static struct obs_source_info dante_input_info = {
	.id = DANTE_INPUT_ID,
	.type = OBS_SOURCE_TYPE_INPUT,
	.output_flags = OBS_SOURCE_AUDIO,
	.get_name = dante_input_name,
	.create = dante_input_create,
	.destroy = dante_input_destroy,
	.icon_type = OBS_ICON_TYPE_AUDIO_INPUT,
};

/* dlink may leave its pump running past its stop wait (it never kills a thread); with this module
 * held loaded for the process, that pump's code stays mapped even after OBS unloads us. */
static void pin_module(void)
{
	Dl_info info;
	if (dladdr((void *)obs_module_load, &info) && info.dli_fname &&
	    dlopen(info.dli_fname, RTLD_NOW | RTLD_NOLOAD | RTLD_NODELETE))
		blog(LOG_INFO, "dante-input start: %s held loaded for the process", info.dli_fname);
	else
		blog(LOG_WARNING, "dante-input start: could not hold the module loaded: %s", dlerror());
}

bool obs_module_load(void)
{
	const char *home = getenv("HOME");
	struct dstr path = {0};
	dstr_printf(&path, "%s/.camera-box/dante-input.toml", home ? home : "");
	if (!home || !os_file_exists(path.array)) {
		blog(LOG_INFO, "dante-input start: no %s; no Dante device and no %s source (as before)",
		     path.array, DANTE_INPUT_ID);
		dstr_free(&path);
		return true;
	}

	dante_input_set_log(on_log);
	char err[512] = {0};
	/* device_rate is set before the first block can need it: the pump may run before
	 * dante_input_start returns, so on_audio skips blocks until it is set. */
	device = dante_input_start(path.array, on_audio, NULL, err, sizeof(err));
	if (device) {
		device_channels = dante_input_channels(device);
		os_atomic_set_long(&device_rate, (long)dante_input_sample_rate(device));
		pin_module();
	} else {
		/* The source kind is still registered, so a scene that names it loads silent, not missing;
		 * the reason is in the log as `dante-input error:`. */
		blog(LOG_ERROR, "dante-input error: the device did not start from %s: %s", path.array, err);
	}
	obs_register_source(&dante_input_info);
	registered = true;
	blog(LOG_INFO, "dante-input start: source kind %s registered (device %s)", DANTE_INPUT_ID,
	     device ? "running" : "NOT running");
	dstr_free(&path);
	return true;
}

void obs_module_unload(void)
{
	if (device) {
		dante_input_stop(device);
		device = NULL;
	}
	if (!registered)
		return;
	pthread_mutex_lock(&sources_mutex);
	size_t left = sources.num;
	if (left == 0)
		da_free(sources);
	pthread_mutex_unlock(&sources_mutex);
	if (left)
		blog(LOG_WARNING, "dante-input stop: %zu source(s) still alive at unload; their list is kept", left);
	blog(LOG_INFO, "dante-input stop: module unloaded");
}

const char *obs_module_name(void)
{
	return "dante-input";
}

const char *obs_module_description(void)
{
	return "Dante audio input through dstack's dlink (obs#1)";
}
