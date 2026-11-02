/* The C ABI of dante-input-ffi (plugins/dante-input/ffi): one dlink device (dstack) per process,
 * RX only, its blocks handed to a callback as float planes (obs#1, camera-box#1485). */
#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

struct dante_input;

/* planes[c] holds `frames` samples of channel c, -1.0 to 1.0. Runs on the device's pump thread. */
typedef void (*dante_input_audio_cb)(void *opaque, const float *const *planes, uint32_t channels, uint32_t frames);
/* level is OBS's (LOG_ERROR 100, LOG_WARNING 200, LOG_INFO 300). */
typedef void (*dante_input_log_cb)(int level, const char *msg);

/* Routes the device's log lines to `cb`. The first call counts. */
void dante_input_set_log(dante_input_log_cb cb);

/* Starts the device of the TOML config at `path`. NULL on failure, the reason in `err`. */
struct dante_input *dante_input_start(const char *path, dante_input_audio_cb cb, void *opaque, char *err,
				      size_t err_len);

/* The device's sample rate, Hz: every block's (the config's sample_rate, 48000 unless set). */
uint32_t dante_input_sample_rate(const struct dante_input *d);

/* The RX channels per block. */
uint32_t dante_input_channels(const struct dante_input *d);

/* Stops the device; no callback runs after this returns. */
void dante_input_stop(struct dante_input *d);

#ifdef __cplusplus
}
#endif
