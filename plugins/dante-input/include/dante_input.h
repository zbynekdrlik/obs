/* The C ABI of dante-input-ffi (plugins/dante-input/ffi): one dlink device (dstack) per process,
 * RX only, its blocks handed to a callback as float planes (obs#1, camera-box#1485). */
#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

struct dante_input;

/* planes[c] holds `frames` samples of channel c, -1.0 to 1.0. Runs on the device's OBS feeder thread.
 * timestamp_ns is the first frame's play time on OBS's clock (os_gettime_ns): dlink's per-block play
 * time plus the frames before it in the block (dstack#295); 0 while the device has no media clock
 * (obs#53). */
typedef void (*dante_input_audio_cb)(void *opaque, const float *const *planes, uint32_t channels, uint32_t frames,
				     uint64_t timestamp_ns);
/* level is OBS's (LOG_ERROR 100, LOG_WARNING 200, LOG_INFO 300). */
typedef void (*dante_input_log_cb)(int level, const char *msg);

/* Routes the device's log lines to `cb`. The first call counts. */
void dante_input_set_log(dante_input_log_cb cb);

/* Starts the device of the TOML config at `path` for OBS running at `obs_rate` Hz (0: unknown).
 * The blocks come at obs_rate unless the config sets sample_rate; a network_rate twice it runs the
 * device at the network's rate with dlink's 2:1 step. NULL on failure, the reason in `err`. */
struct dante_input *dante_input_start(const char *path, uint32_t obs_rate, dante_input_audio_cb cb, void *opaque,
				      char *err, size_t err_len);

/* The blocks' sample rate, Hz (OBS's rate unless the config sets one). */
uint32_t dante_input_sample_rate(const struct dante_input *d);

/* The RX channels per block. */
uint32_t dante_input_channels(const struct dante_input *d);

/* The OBS audio track (from 1) the device sends on its TX channels; 0 without TX (obs#41). */
uint32_t dante_input_tx_track(const struct dante_input *d);

/* The rate, Hz, to hand dante_input_tx_push the track at; 0 without TX. */
uint32_t dante_input_tx_rate(const struct dante_input *d);

/* Hands the TX `frames` frames of the track as `channels` float planes (left, right; one plane goes
 * out on both). OBS's audio thread only, at dante_input_tx_rate. */
void dante_input_tx_push(const struct dante_input *d, const float *const *planes, uint32_t channels,
			 uint32_t frames);

/* Stops the device; no callback runs after this returns. */
void dante_input_stop(struct dante_input *d);

#ifdef __cplusplus
}
#endif
