/******************************************************************************
	camera-box issue 1387 slice 3: the JPEG XS PROGRAM output of the strih OBS.

	The strih program reaches stream-snv as NDI (`NDI 2ME PGM`, SpeedHQ). This output sends the
	same program at JPEG XS quality over the camera-box link (our own UDP, the 16-byte header,
	each slice on the wire the moment it is coded, the automatic bpp on the receiver's report),
	BESIDE the NDI output, which stays (owner decision 6488036928).

	- It takes the program's raw video the way the NDI output does (`raw_video` on libobs' video
	  output thread) in the canvas format as it is (NV12 / I420 / I422 / I444: JPEG XS codes
	  4:2:0, 4:2:2 and 4:4:4 as they are).
	- Every frame is stamped with genlock_emit_timecode_100ns (genlock-emit-timecode.h), the ONE
	  stamp the NDI output puts on the same frame, so the stream side's camera_box_jxs_source
	  places it on the per-second grid exactly like `NDI 2ME PGM`.
	- The frame goes to jxs/send-ffi, the C ABI over camera-box's own sender (src/jxs_send*): one
	  copy into one of two reusable pictures on this thread, never a wait; with both in use the
	  frame is dropped and counted (dropped_busy). Coding, slicing, UDP GSO and the auto bpp run on
	  the sender's own two threads.

	The output is created by the plugin at FINISHED_LOADING (obs-websocket can start, stop and
	configure an existing output, never create one) from <module config>/jxs-output.json, and is
	started only when that file says "enabled": true. It is stopped before a profile change
	(a video reset refuses active outputs) and at exit.

	camera-box#1387 (strih-snv reboot 4.11.2026 20:49): a start is RETRIED (while it retries the output
	is inactive, so obs-websocket StopOutput answers OutputNotRunning: enabled=false in jxs-output.json
	and a profile change or restart is what ends a retry). OBS came up before the
	network, the sender could not connect ("Network is unreachable"), the output logged "did not
	start" once and the stream program stayed black for 30 min. Every start marks the output
	WANTED from jxs-output.json enabled=true at load, or from a start that succeeded (a failed
	obs-websocket StartOutput alone arms nothing); a stop clears it (StopOutput, the profile change,
	exit). A supervisor thread looks twice a second and starts a wanted output that is not active,
	2 s after its last failed try (jxs-output-decide.h); a start error is logged once per outage,
	the start after one names its tries. A LIVE sender that died (a panic poisoned it, its threads
	are gone, or no frame handed to it went out whole for two 5 s windows) is stopped by the
	supervisor, keeping `wanted`, and started again by the retry. A send error is no death: vstack
	counts a frame with failed sends as sent whole, so a receiver that is off restarts nothing (and
	a source-IP change is not seen either; strih runs a static IP). A sender that REFUSES every
	frame (a geometry the codec cannot take) is not restarted: a restart would refuse it again.
	libobs ignores a stop while the output is stopping or inactive, so an operator's StopOutput
	during a restart's stop or its 2 s gap before the next start does nothing, and the output comes
	back: after a restart, stop it once it runs again.

	Built only where the strih Linux build sets CAMERA_BOX_JXS_SOURCE (linux-genlock.yml, the
	strih variant), like the source: the sender is Linux-only (UDP GSO).
******************************************************************************/

#include <obs-module.h>
#include <util/bmem.h>
#include <util/platform.h>
#include <util/threading.h>

#include <atomic>
#include <chrono>
#include <cinttypes>
#include <condition_variable>
#include <cstdio>
#include <mutex>
#include <string>
#include <thread>

#include "plugin-support.h"
#include "camera_box_jxs_send.h"
#include "genlock-emit-timecode.h"
#include "jxs-output-decide.h"
#include "jxs-state-table.h"

#define JXS_OUTPUT_ID "camera_box_jxs_output"
#define JXS_OUTPUT_NAME "camera-box JPEG XS program"

#define PROP_JXS_TARGET "jxs_target"
#define PROP_JXS_STREAM_ID "jxs_stream_id"
#define PROP_JXS_BPP_MIN "jxs_bpp_min"
#define PROP_JXS_BPP_MAX "jxs_bpp_max"
#define PROP_JXS_CORES "jxs_cores"
#define PROP_JXS_PACE_MBPS "jxs_pace_mbps"

/* camera-box#1387: true only on the supervisor's thread while it stops a dead sender. libobs runs
 * info.stop synchronously on the stopping thread for this raw output, so jxs_output_stop reads it
 * as "the supervisor's own restart: keep `wanted`". Per thread, so nothing is shared. */
static thread_local bool t_supervisor_stop = false;

struct jxs_output {
	obs_output_t *output = nullptr;
	/* settings (update() may run on any thread; start() reads them) */
	std::mutex settings_mutex;
	std::string target;
	std::string cores;
	uint8_t stream_id = CB_JXS_OUT_STREAM_ID_DEFAULT;
	uint8_t bpp_min = 0;
	uint8_t bpp_max = 0;
	uint32_t pace_mbps = CB_JXS_OUT_PACE_MBPS_DEFAULT;
	/* running state */
	std::mutex tx_mutex;
	cbjxs_sender *tx = nullptr;
	std::atomic<bool> started{false};
	/* camera-box#1387: jxs-output.json enabled=true or a start that succeeded, and no stop since --
	 * the supervisor retries it (a failed obs-websocket StartOutput alone arms nothing) */
	std::atomic<bool> wanted{false};
	/* the live sender died (cb_jxs_out_window_dead / _stats_stale): the supervisor restarts it */
	std::atomic<bool> dead{false};

	/* raw_video's health counters (tx_mutex held) */
	uint32_t dead_windows = 0;
	bool poisoned = false;
	uint64_t last_window_ns = 0;
	/* a start() in flight: the supervisor and obs-websocket StartOutput may both call it, and
	 * libobs does not serialise two starts of an inactive output -- the second returns at once, so
	 * no two senders exist. (libobs' own last-error handling in obs_output_actual_start can still
	 * race between two such callers; that predates the retry.) */
	std::atomic<bool> starting{false};
	/* the last start error logged (cb_jxs_out_error_new), start() only */
	char last_error[CB_JXS_OUT_ERROR_LEN] = "";
	uint32_t send_format = 0;
	double framerate = 0.0;
	/* raw_video's own counters of the current status window */
	uint64_t offered = 0;
	uint64_t taken = 0;
	uint64_t not_taken = 0;
	uint64_t refused = 0;
	uint64_t stamp_dup = 0;
	/* the last stamp handed to the sender (0 = none), the duplicate guard */
	int64_t last_stamp_100ns = 0;
};

/* The sender's own lines (link ON, encoder open, every bpp step, the wedge alarm) into the OBS
 * log; its 5 s status line is logged by the output itself (jxs_output_log_status). */
static void jxs_output_forward_log(int level, const char *message)
{
	if (!cb_jxs_out_forward_line(message))
		return;
	const int obs_level = level == CBJXS_SEND_LOG_ERROR  ? LOG_ERROR
			      : level == CBJXS_SEND_LOG_WARN ? LOG_WARNING
							     : LOG_INFO;
	obs_log(obs_level, "jxs-output: %s", message);
}

/* The sender's picture format for the canvas format; 0 = not one JPEG XS codes. */
static uint32_t jxs_output_send_format(enum video_format format)
{
	switch (format) {
	case VIDEO_FORMAT_NV12:
		return CBJXS_SEND_NV12;
	case VIDEO_FORMAT_I420:
		return CBJXS_SEND_I420;
	case VIDEO_FORMAT_I422:
		return CBJXS_SEND_I422;
	case VIDEO_FORMAT_I444:
		return CBJXS_SEND_I444;
	default:
		return 0;
	}
}

static const char *jxs_output_getname(void *)
{
	return "Camera-box JPEG XS program output (issue 1387)";
}

static void jxs_output_getdefaults(obs_data_t *settings)
{
	obs_data_set_default_string(settings, PROP_JXS_TARGET, "");
	obs_data_set_default_int(settings, PROP_JXS_STREAM_ID, CB_JXS_OUT_STREAM_ID_DEFAULT);
	obs_data_set_default_int(settings, PROP_JXS_BPP_MIN, 0);
	obs_data_set_default_int(settings, PROP_JXS_BPP_MAX, 0);
	obs_data_set_default_string(settings, PROP_JXS_CORES, "");
	obs_data_set_default_int(settings, PROP_JXS_PACE_MBPS, CB_JXS_OUT_PACE_MBPS_DEFAULT);
}

static obs_properties_t *jxs_output_getproperties(void *)
{
	obs_properties_t *props = obs_properties_create();
	obs_properties_add_text(props, PROP_JXS_TARGET, "Target (host:port, IPv4)", OBS_TEXT_DEFAULT);
	obs_properties_add_int(props, PROP_JXS_STREAM_ID, "Stream id (outside the CAM range)", 1, 255, 1);
	obs_properties_add_int(props, PROP_JXS_BPP_MIN, "Lowest bpp (0 = 4)", 0, CB_JXS_OUT_MAX_BPP, 1);
	obs_properties_add_int(props, PROP_JXS_BPP_MAX, "Highest bpp (0 = 8; = lowest: fixed)", 0,
			       CB_JXS_OUT_MAX_BPP, 1);
	obs_properties_add_text(props, PROP_JXS_CORES, "Encoder cores (cpulist, empty = all)", OBS_TEXT_DEFAULT);
	obs_properties_add_int(props, PROP_JXS_PACE_MBPS, "Send pace, Mb/s of wire (0 = automatic from the receiver's link)", 0,
			       CB_JXS_OUT_PACE_MBPS_MAX, 50);
	return props;
}

/* Stores the settings; they apply at the next start. */
static void jxs_output_update(void *data, obs_data_t *settings)
{
	auto o = static_cast<jxs_output *>(data);
	std::lock_guard<std::mutex> lock(o->settings_mutex);
	o->target = obs_data_get_string(settings, PROP_JXS_TARGET);
	o->cores = obs_data_get_string(settings, PROP_JXS_CORES);
	o->stream_id = cb_jxs_out_stream_id(obs_data_get_int(settings, PROP_JXS_STREAM_ID));
	o->bpp_min = cb_jxs_out_bpp(obs_data_get_int(settings, PROP_JXS_BPP_MIN));
	o->bpp_max = cb_jxs_out_bpp(obs_data_get_int(settings, PROP_JXS_BPP_MAX));
	o->pace_mbps = cb_jxs_out_pace_mbps(obs_data_get_int(settings, PROP_JXS_PACE_MBPS));
}

static void *jxs_output_create(obs_data_t *settings, obs_output_t *output)
{
	auto o = new jxs_output();
	o->output = output;
	jxs_output_update(o, settings);
	return o;
}

/* A start's failure: the output's last error, logged only when it differs from the last one logged
 * (camera-box#1387: the supervisor retries every 2 s, an outage logs once). */
static bool jxs_output_start_failed(jxs_output *o, const std::string &msg, const char *last_error)
{
	if (cb_jxs_out_error_new(o->last_error, sizeof o->last_error, msg.c_str()))
		obs_log(LOG_ERROR, "jxs-output '%s': %s -- retried every 2 s while it is wanted (issue 1387)",
			obs_output_get_name(o->output), msg.c_str());
	obs_output_set_last_error(o->output, last_error);
	return false;
}

static bool jxs_output_start(void *data)
{
	auto o = static_cast<jxs_output *>(data);
	const char *name = obs_output_get_name(o->output);
	if (o->started.load() || o->starting.exchange(true))
		return false;
	struct starting_done {
		std::atomic<bool> &flag;
		~starting_done() { flag = false; }
	} done{o->starting};
	obs_output_set_last_error(o->output, "");
	video_t *video = obs_output_video(o->output);
	if (!video)
		return jxs_output_start_failed(o, "no program video -- not started", "no program video");
	const uint32_t send_format = jxs_output_send_format(video_output_get_format(video));
	if (send_format == 0)
		return jxs_output_start_failed(
			o,
			"the canvas pixel format " + std::to_string((int)video_output_get_format(video)) +
				" is not one JPEG XS codes (NV12, I420, I422, I444) -- not started",
			"the canvas pixel format is not NV12/I420/I422/I444");
	const struct video_output_info *vinfo = video_output_get_info(video);
	/* an sRGB canvas is converted to YUV with the same BT.709 matrix */
	const bool cs_709 = vinfo && (vinfo->colorspace == VIDEO_CS_709 || vinfo->colorspace == VIDEO_CS_SRGB);
	const bool range_partial = vinfo && vinfo->range != VIDEO_RANGE_FULL;
	if (!cb_jxs_out_colour_ok(cs_709, range_partial))
		return jxs_output_start_failed(
			o,
			"the canvas is colour space " + std::to_string(vinfo ? (int)vinfo->colorspace : -1) + " range " +
				std::to_string(vinfo ? (int)vinfo->range : -1) +
				", the receiving source shows BT.709 limited range -- not started",
			"the canvas is not BT.709 limited range");
	const double framerate = video_output_get_frame_rate(video);
	std::string target, cores;
	cbjxs_send_config cfg = {};
	{
		std::lock_guard<std::mutex> lock(o->settings_mutex);
		target = o->target;
		cores = o->cores;
		cfg.stream_id = o->stream_id;
		cfg.bpp_min = o->bpp_min;
		cfg.bpp_max = o->bpp_max;
		cfg.pace_mbps = o->pace_mbps;
	}
	cfg.target = target.c_str();
	cfg.cores = cores.c_str();
	cfg.emit_fps = cb_jxs_out_fps(framerate);
	char err[256] = "";
	cbjxs_sender *tx = cbjxs_send_open(&cfg, err, sizeof err);
	if (!tx)
		return jxs_output_start_failed(o, "cannot start the sender to '" + target + "': " + err, err);
	{
		std::lock_guard<std::mutex> lock(o->tx_mutex);
		o->tx = tx;
		o->send_format = send_format;
		o->framerate = framerate;
		o->offered = o->taken = o->not_taken = o->refused = o->stamp_dup = 0;
		o->last_stamp_100ns = 0;
		o->dead_windows = 0;
		o->poisoned = false;
		o->last_window_ns = os_gettime_ns();
	}
	o->dead = false;
	o->started = true;
	if (!obs_output_begin_data_capture(o->output, 0)) {
		o->started = false;
		{
			std::lock_guard<std::mutex> lock(o->tx_mutex);
			o->tx = nullptr;
		}
		cbjxs_send_close(tx);
		return jxs_output_start_failed(o, "the program capture did not start", "the program capture did not start");
	}
	cb_jxs_out_error_clear(o->last_error);
	o->wanted = true;
	/* every frame carries the genlock wall stamp: the LOCK indicator counts it as a sender */
	obs_output_set_genlock_wall_stamping(o->output, true);
	/* the JPEG XS indicator's row (issue 1387): published only while the output runs */
	cb_jxs_state().set_entry(o, CB_JXS_STATE_OUTPUT, name, "to " + target);
	obs_log(LOG_INFO,
		"jxs-output '%s': started -- the program %ux%u %s at %.2f fps to %s (stream id %u, bpp %u..%u, 0 = the sender's default; cores '%s', empty = all; pace %u Mb/s, 0 = automatic from the receiver's link), each frame stamped with the genlock emit timecode of the NDI output, send ABI %u (issue 1387)",
		name, video_output_get_width(video), video_output_get_height(video),
		get_video_format_name(video_output_get_format(video)), framerate, target.c_str(),
		(unsigned)cfg.stream_id, (unsigned)cfg.bpp_min, (unsigned)cfg.bpp_max, cores.c_str(),
		(unsigned)cfg.pace_mbps,
		cbjxs_send_abi_version());
	return true;
}

/* The 5 s status line: the sender's window plus raw_video's own counters (tx_mutex held). */
static void jxs_output_log_status(jxs_output *o, const cbjxs_send_stats &st)
{
	obs_log(LOG_INFO,
		"jxs-output '%s': fps=%.2f frames=%" PRIu64 " dropped_busy=%" PRIu64
		" offered=%" PRIu64 " taken=%" PRIu64 " not_taken=%" PRIu64 " refused=%" PRIu64
		" stamp_dup=%" PRIu64
		" encode_ms_avg/max=%.2f/%.2f first_slice_ms_avg=%.2f copy_ms_avg/max=%.2f/%.2f bpp=%u auto=%s"
		" reports=%" PRIu64 " report_age_s=%.1f send_errors=%" PRIu64 " encode_errors=%" PRIu64
		" pace_mbps=%u wire_ms_avg/max=%.2f/%.2f pace_src=%s rx_link_mbps=%u tx_link_mbps=%u"
		" path=%s cong_pct=%u queue_ms=%.1f fec_sent=%" PRIu64 " fec_repaired=%" PRIu64 " (issue 1387)",
		obs_output_get_name(o->output), st.window_s > 0.0 ? (double)st.frames / st.window_s : 0.0,
		st.frames, st.dropped_busy, o->offered, o->taken, o->not_taken, o->refused, o->stamp_dup,
		st.encode_ms_avg,
		st.encode_ms_max, st.first_slice_ms_avg, st.copy_ms_avg, st.copy_ms_max, st.bpp,
		st.auto_bpp ? "on" : "off", st.reports, st.report_age_s, st.send_errors, st.encode_errors,
		st.pace_mbps, st.wire_ms_avg, st.wire_ms_max, cb_jxs_out_pace_source(st.pace_source), st.rx_link_mbps,
		st.own_link_mbps, cb_jxs_out_path_state(st.path_state), st.congestion_pct,
		st.queue_delay_us == UINT32_MAX ? -1.0 : st.queue_delay_us / 1000.0, st.fec_sent, st.fec_repaired);
	const double fps = st.window_s > 0.0 ? (double)st.frames / st.window_s : 0.0;
	/* the indicator's errors: encode_errors already counts a refused picture (send-ffi header) */
	cb_jxs_state().publish_window(o, (uint32_t)(fps * 100.0 + 0.5), st.bpp * 100u, 0, 0, st.dropped_busy,
				      st.send_errors + st.encode_errors,
				      st.report_age_s < 0.0 ? -1 : (int64_t)(st.report_age_s * 1000.0));
	o->offered = o->taken = o->not_taken = o->refused = o->stamp_dup = 0;
}

/* libobs' video output thread: one copy into the sender, never a wait. */
static void jxs_output_rawvideo(void *data, struct video_data *frame)
{
	auto o = static_cast<jxs_output *>(data);
	if (!o->started.load() || !frame)
		return;
	std::lock_guard<std::mutex> lock(o->tx_mutex);
	if (!o->tx)
		return;
	video_t *video = obs_output_video(o->output);
	cbjxs_send_picture pic = {};
	pic.format = o->send_format;
	pic.width = video_output_get_width(video);
	pic.height = video_output_get_height(video);
	for (int i = 0; i < 3; i++) {
		pic.planes[i] = frame->data[i];
		pic.strides[i] = frame->linesize[i];
	}
	/* the NDI output's stamp function on the frame's own render tick (issue 1367), so this
	 * frame gets the same stamp the NDI output put on it */
	pic.timecode_100ns = genlock_emit_timecode_100ns(o->framerate, frame->timestamp);
	o->offered++;
	if (!cb_jxs_out_stamp_fresh(o->last_stamp_100ns, pic.timecode_100ns)) {
		/* a second frame in one slot: the receiver would drop it unseen, count it here */
		o->stamp_dup++;
		return;
	}
	const int r = cbjxs_send_frame(o->tx, &pic);
	if (r == 1) {
		/* only a frame really handed over claims its slot: after a busy drop the slot's next
		 * frame may still go */
		o->last_stamp_100ns = pic.timecode_100ns;
		o->taken++;
		cb_jxs_state().note_frame(o, os_gettime_ns());
	} else if (r == 0) {
		o->not_taken++;
	} else {
		if (r == -2)
			o->poisoned = true;
		if (o->refused++ == 0) {
			char e[256] = "";
			cbjxs_send_take_error(o->tx, e, sizeof e);
			obs_log(LOG_WARNING, "jxs-output '%s': frame not sent (%d): %s (issue 1387)",
				obs_output_get_name(o->output), r, e);
		}
	}
	cbjxs_send_stats st = {};
	const uint64_t now = os_gettime_ns();
	bool dead = false;
	const char *why = nullptr;
	if (cbjxs_send_get_stats(o->tx, &st) == 1) {
		o->last_window_ns = now;
		if (cb_jxs_out_window_dead(&o->dead_windows, o->taken + o->not_taken, st.frames, o->poisoned)) {
			dead = true;
			why = o->poisoned ? "the sender is poisoned (a panic in the library)"
					  : "two 5 s windows with frames handed to the sender and none sent whole";
		}
		jxs_output_log_status(o, st);
	} else if (o->poisoned) {
		dead = true;
		why = "the sender is poisoned (a panic in the library)";
	} else if (cb_jxs_out_stats_stale(o->last_window_ns, now, o->taken + o->not_taken)) {
		dead = true;
		why = "no status window from the sender for 15 s (its threads are gone)";
	}
	if (dead && !o->dead.exchange(true))
		obs_log(LOG_WARNING, "jxs-output '%s': %s -- the sender is restarted (issue 1387)",
			obs_output_get_name(o->output), why);
}

static void jxs_output_stop(void *data, uint64_t)
{
	auto o = static_cast<jxs_output *>(data);
	/* camera-box#1387: a stop (StopOutput, the profile change, exit) ends the retry; the
	 * supervisor's own stop of a dead sender keeps it */
	if (!t_supervisor_stop)
		o->wanted = false;
	if (!o->started.exchange(false))
		return;
	obs_output_set_genlock_wall_stamping(o->output, false);
	/* libobs ends the capture on its own thread, so a raw_video can still run after this returns:
	 * tx_mutex + tx = nullptr below is what keeps it off a closed sender (keep both) */
	obs_output_end_data_capture(o->output);
	cbjxs_sender *tx = nullptr;
	{
		std::lock_guard<std::mutex> lock(o->tx_mutex);
		tx = o->tx;
		o->tx = nullptr;
	}
	/* bounded: the sender's threads stop within ~2 s even with a wedged codec call */
	cbjxs_send_close(tx);
	cb_jxs_state().remove(o);
	obs_log(LOG_INFO, "jxs-output '%s': stopped (issue 1387)", obs_output_get_name(o->output));
}

static void jxs_output_destroy(void *data)
{
	auto o = static_cast<jxs_output *>(data);
	if (o->started.load())
		jxs_output_stop(o, 0);
	delete o;
}

struct obs_output_info create_jxs_output_info()
{
	/* the sender's own lines reach the OBS log (installed once, here at registration) */
	if (!cbjxs_send_set_log(jxs_output_forward_log))
		obs_log(LOG_WARNING,
			"jxs-output: another Rust log subscriber owns the process -- the sender's own lines are not logged (issue 1387)");
	struct obs_output_info info = {};
	info.id = JXS_OUTPUT_ID;
	info.flags = OBS_OUTPUT_VIDEO;
	info.get_name = jxs_output_getname;
	info.get_defaults = jxs_output_getdefaults;
	info.get_properties = jxs_output_getproperties;
	info.create = jxs_output_create;
	info.update = jxs_output_update;
	info.start = jxs_output_start;
	info.stop = jxs_output_stop;
	info.destroy = jxs_output_destroy;
	info.raw_video = jxs_output_rawvideo;
	return info;
}

/* The one program output, created at FINISHED_LOADING. */
static obs_output_t *g_jxs_output = nullptr;

/* camera-box#1387: the start retry supervisor (one thread while the output exists). */
static std::mutex g_retry_mutex;
static std::condition_variable g_retry_cv;
static bool g_retry_quit = false;
static std::thread g_retry_thread;
static struct cb_jxs_out_retry g_retry = {};

/* One start, its outcome noted; logs the tries a start after failures took. */
static void jxs_output_try_start(obs_output_t *output, uint64_t now_ns)
{
	const uint32_t fails = g_retry.fails;
	const bool ok = obs_output_start(output);
	cb_jxs_out_retry_note(&g_retry, ok, now_ns);
	if (ok && fails > 0)
		obs_log(LOG_INFO, "jxs-output: '%s' started after %u failed tries, 2 s apart (issue 1387)",
			JXS_OUTPUT_NAME, (unsigned)fails);
}

/* g_retry is this thread's (and init's, before the thread exists); g_retry_mutex guards only the
 * quit flag and the wait, never a start: a start (the sender's open) runs unlocked, so deinit never
 * waits on it for the lock, only in the join. */
static void jxs_output_retry_loop(obs_output_t *output)
{
	os_set_thread_name("jxs-output-retry");
	for (;;) {
		{
			std::unique_lock<std::mutex> lock(g_retry_mutex);
			g_retry_cv.wait_for(lock, std::chrono::milliseconds(500), [] { return g_retry_quit; });
			if (g_retry_quit)
				return;
		}
		auto o = static_cast<jxs_output *>(obs_obj_get_data(output));
		const bool wanted = o && o->wanted.load();
		const uint64_t now = os_gettime_ns();
		const bool active = obs_output_active(output);
		if (o && active && wanted && o->dead.load()) {
			/* the live sender died: stop it (keeping `wanted`), the retry starts it again */
			t_supervisor_stop = true;
			obs_output_stop(output);
			t_supervisor_stop = false;
			cb_jxs_out_retry_restart(&g_retry, now);
			continue;
		}
		if (cb_jxs_out_retry_due(&g_retry, wanted, active, now))
			jxs_output_try_start(output, now);
	}
}

void camera_box_jxs_output_init()
{
	if (g_jxs_output)
		return;
	char *path = obs_module_config_path("jxs-output.json");
	obs_data_t *file = path ? obs_data_create_from_json_file_safe(path, "bak") : nullptr;
	obs_data_t *settings = obs_data_create();
	if (file)
		obs_data_apply(settings, file);
	const bool enabled = file && obs_data_get_bool(file, "enabled");
	g_jxs_output = obs_output_create(JXS_OUTPUT_ID, JXS_OUTPUT_NAME, settings, nullptr);
	obs_log(LOG_INFO,
		"jxs-output: '%s' created from %s (%s; enabled=%s, target '%s') -- obs-websocket StartOutput / StopOutput / SetOutputSettings drive it (issue 1387)",
		JXS_OUTPUT_NAME, path ? path : "(no config path)", file ? "read" : "absent: defaults",
		enabled ? "true" : "false", obs_data_get_string(settings, PROP_JXS_TARGET));
	if (g_jxs_output) {
		g_retry = {};
		{
			std::lock_guard<std::mutex> lock(g_retry_mutex);
			g_retry_quit = false;
		}
		/* a failed first start (the network not up yet) is the supervisor's to retry */
		if (enabled) {
			if (auto o = static_cast<jxs_output *>(obs_obj_get_data(g_jxs_output)))
				o->wanted = true;
			jxs_output_try_start(g_jxs_output, os_gettime_ns());
		}
		g_retry_thread = std::thread(jxs_output_retry_loop, g_jxs_output);
	}
	obs_data_release(settings);
	obs_data_release(file);
	bfree(path);
}

void camera_box_jxs_output_deinit()
{
	if (!g_jxs_output)
		return;
	/* the supervisor first: no start may race the stop and release below */
	{
		std::lock_guard<std::mutex> lock(g_retry_mutex);
		g_retry_quit = true;
	}
	g_retry_cv.notify_all();
	if (g_retry_thread.joinable())
		g_retry_thread.join();
	obs_output_stop(g_jxs_output);
	obs_output_release(g_jxs_output);
	g_jxs_output = nullptr;
}
