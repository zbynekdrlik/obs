/* camera-box issue 1387 slice 3: the pure decisions of the JPEG XS program output (jxs-output.cpp).
 *
 * C, header-only, no OBS or library calls: tests/distroav_jxs_output_1387.rs includes this very
 * file into a truth-table program and runs it under `cargo test` (CI is otherwise the first
 * compiler of the plugin). */
#pragma once

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

/* The program's stream id on the wire: outside the CAM<N> range of the camboxes, so the stream
 * side's source can take it by id. */
#define CB_JXS_OUT_STREAM_ID_DEFAULT 100u
/* The highest bpp the wire carries (camera_box::jxs_send::MAX_BPP). */
#define CB_JXS_OUT_MAX_BPP 12
#define CB_JXS_OUT_STATUS_PREFIX "jxs send: fps="

/* The stream id setting: 1..255, anything else the default. */
static inline uint8_t cb_jxs_out_stream_id(long long v)
{
	return (v >= 1 && v <= 255) ? (uint8_t)v : (uint8_t)CB_JXS_OUT_STREAM_ID_DEFAULT;
}

/* A bpp bound setting: 0 = the sender's default, 1..12, anything else 0. */
static inline uint8_t cb_jxs_out_bpp(long long v)
{
	return (v >= 0 && v <= CB_JXS_OUT_MAX_BPP) ? (uint8_t)v : 0;
}

/* The send pacing default: 0 = AUTOMATIC (send ABI 3, issue 1387, owner 29.10.2026: no number
 * that a network reconfiguration silently breaks). The sender paces to 90 % of the receiver's link
 * from its report when that link is slower than strih's own (stream-snv's 1 GbE port -> 900, the
 * value the 23:35 live run proved with 0 lost slices; a 2.5 GbE PP receiver -> 2250), at line rate
 * when it is as fast. A value above 0 is a fixed override. */
#define CB_JXS_OUT_PACE_MBPS_DEFAULT 0
/* The sender's bounds (camera_box::jxs_link::pace::PACE_MBPS_MIN / _MAX); 0 = automatic. */
#define CB_JXS_OUT_PACE_MBPS_MIN 100
#define CB_JXS_OUT_PACE_MBPS_MAX 100000

/* The pace setting: 0 (or below) = automatic, else clamped into the sender's bounds, so a setting
 * never makes the start fail. */
static inline uint32_t cb_jxs_out_pace_mbps(long long v)
{
	if (v <= 0)
		return 0;
	if (v < CB_JXS_OUT_PACE_MBPS_MIN)
		return CB_JXS_OUT_PACE_MBPS_MIN;
	if (v > CB_JXS_OUT_PACE_MBPS_MAX)
		return CB_JXS_OUT_PACE_MBPS_MAX;
	return (uint32_t)v;
}

/* The status line's word for cbjxs_send_stats.pace_source (CBJXS_SEND_PACE_*; the Rust
 * PaceSource tokens). */
static inline const char *cb_jxs_out_pace_source(uint32_t code)
{
	switch (code) {
	case 0:
		return "line-rate";
	case 1:
		return "fixed";
	case 2:
		return "receiver-link";
	case 3:
		return "start-spread";
	case 4:
		return "congestion";
	default:
		return "unknown";
	}
}

/* The status line's word for cbjxs_send_stats.path_state (CBJXS_SEND_PATH_*; the Rust PathState
 * tokens). */
static inline const char *cb_jxs_out_path_state(uint32_t code)
{
	switch (code) {
	case 0:
		return "unknown";
	case 1:
		return "clean";
	case 2:
		return "hold";
	case 3:
		return "overuse";
	default:
		return "?";
	}
}

/* The canvas frame rate the sender is told (it sends every frame at 30 or below, the 30 fps grid
 * frames above), rounded; a non-positive or absurd rate is 0 (the open refuses nothing for it,
 * the sender then sends every frame). */
static inline uint32_t cb_jxs_out_fps(double fps)
{
	if (!(fps > 0.0) || fps > 1000.0)
		return 0;
	return (uint32_t)(fps + 0.5);
}

/* Whether a frame's stamp may go: never the stamp just sent. The stamp is the frame's own render
 * tick (genlock_emit_timecode_100ns, the NDI output's function, issue 1367), so a repeat can only
 * come from a frame whose tick maps into an already-sent slot (a clamp to the send floor, a wall
 * step); the receiver would drop the second as a duplicate, unseen by the sender. A stamp BEHIND the last
 * one goes: that is a wall-clock step back (the nightly dantesync date step), which the receiver
 * resyncs on; holding frames until the clock passes the old stamp would cost up to ~200 ms of
 * program. 0 = none sent. */
static inline bool cb_jxs_out_stamp_fresh(int64_t last_sent_100ns, int64_t stamp_100ns)
{
	return last_sent_100ns == 0 || stamp_100ns != last_sent_100ns;
}

/* The receiving source shows the program as BT.709 limited range (jxs-source.cpp): the output
 * starts only on a canvas that is that (the strih profile pins 709 / Partial). `cs_709` /
 * `range_partial` are the caller's reads of the canvas (an sRGB canvas counts as 709, its YUV
 * conversion uses the 709 matrix; a default range counts as partial). */
static inline bool cb_jxs_out_colour_ok(bool cs_709, bool range_partial)
{
	return cs_709 && range_partial;
}

/* Whether a line the sender logs goes on to the OBS log: everything but the sender's own 5 s
 * status line, which the output logs itself, named, from the same window (its counters plus the
 * output's own). */
static inline bool cb_jxs_out_forward_line(const char *message)
{
	if (!message)
		return false;
	return strncmp(message, CB_JXS_OUT_STATUS_PREFIX, strlen(CB_JXS_OUT_STATUS_PREFIX)) != 0;
}

/* ---- camera-box#1387 (strih-snv reboot 4.11.2026): the start retry ----
 *
 * OBS started before the network: the sender's connect failed ("Network is unreachable"), the
 * output logged "did not start" once and stayed off for good. The output's supervisor thread now
 * looks twice a second and starts a WANTED output (a start was asked: jxs-output.json enabled=true,
 * or obs-websocket StartOutput; a stop clears it) that is not active, 2 s after its last failed try.
 * A start error is logged once per outage (its text changes or it follows a success). */

#define CB_JXS_OUT_RETRY_NS 2000000000ULL
#define CB_JXS_OUT_ERROR_LEN 256

struct cb_jxs_out_retry {
	uint32_t fails;   /* failed starts since the last success */
	uint64_t next_ns; /* the earliest next try (0 = at once) */
};

/* Whether the supervisor starts the output now. */
static inline bool cb_jxs_out_retry_due(const struct cb_jxs_out_retry *r, bool wanted, bool active,
					uint64_t now_ns)
{
	return wanted && !active && now_ns >= r->next_ns;
}

/* One start's outcome at `now_ns`: a success resets, a failure schedules the next try 2 s on. */
static inline void cb_jxs_out_retry_note(struct cb_jxs_out_retry *r, bool ok, uint64_t now_ns)
{
	if (ok) {
		r->fails = 0;
		r->next_ns = 0;
	} else {
		r->fails++;
		r->next_ns = now_ns + CB_JXS_OUT_RETRY_NS;
	}
}

/* Whether a start error is logged: only when its text differs from the last one logged (`last`,
 * `len` bytes, kept truncated). */
static inline bool cb_jxs_out_error_new(char *last, size_t len, const char *err)
{
	if (!last || len == 0)
		return true;
	if (!err)
		err = "";
	/* `last` holds at most len - 1 bytes of the logged error: equal there = the same error */
	if (strncmp(last, err, len - 1) == 0)
		return false;
	strncpy(last, err, len - 1);
	last[len - 1] = '\0';
	return true;
}

/* A success: the next outage's first error is logged again. */
static inline void cb_jxs_out_error_clear(char *last)
{
	if (last)
		last[0] = '\0';
}

/* ---- camera-box#1387: a LIVE sender that died ----
 *
 * vstack's sender never stops on a send error, but it can die: a panic inside the library poisons
 * it (every frame -2), a panicked send thread leaves every frame not taken, a source-IP change
 * fails every send (the connected socket keeps its old source). Each shows as frames offered and
 * none sent whole; a receiver that is off still sends whole frames (ECONNREFUSED only counts
 * send_errors). The output then restarts its sender (stop + the 2 s start retry). */

#define CB_JXS_OUT_DEAD_WINDOWS 2U
/* No new 5 s status window for this long while frames are offered: the sender's threads are gone. */
#define CB_JXS_OUT_STATS_STALE_NS 15000000000ULL

/* One status window: `offered` frames handed to the output, `frames_sent` sent whole by the
 * sender, `poisoned` = a frame came back -2. True = restart the sender. `dead_windows` counts the
 * windows in a row with frames offered and none sent. */
static inline bool cb_jxs_out_window_dead(uint32_t *dead_windows, uint64_t offered, uint64_t frames_sent,
					  bool poisoned)
{
	if (poisoned)
		return true;
	if (offered == 0 || frames_sent > 0) {
		*dead_windows = 0;
		return false;
	}
	return ++*dead_windows >= CB_JXS_OUT_DEAD_WINDOWS;
}

/* True = the sender has handed out no status window for CB_JXS_OUT_STATS_STALE_NS while frames
 * were offered (its threads are gone, so no window can show the death). */
static inline bool cb_jxs_out_stats_stale(uint64_t last_window_ns, uint64_t now_ns, uint64_t offered)
{
	return offered > 0 && last_window_ns > 0 && now_ns >= last_window_ns + CB_JXS_OUT_STATS_STALE_NS;
}
