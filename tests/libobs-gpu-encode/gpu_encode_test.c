/* camera-box#1387 (obs#1): libobs texture encoding across recordings, through real libobs and
 * libobs-opengl (CI runs it under xvfb).
 *
 * With texture NVENC, every StartRecord made a new encode thread and every StopRecord entered
 * graphics from the output's stop thread. A fresh thread's first GL MakeCurrent under the
 * graphics lock took 52-60 ms on NVIDIA and the render tick lost a frame. The test registers a
 * texture encoder of its own (the test double of obs-nvenc's texture path; libobs itself is
 * real) and checks from the outside:
 *
 *   - two recordings encode on the same thread;
 *   - a stop completes while another thread holds the graphics lock (it takes no graphics);
 *   - the second recording's first packet is a new frame, not one left from the first;
 *   - a waiting graphics task runs on the graphics thread, and on its caller when there is no
 *     graphics thread (it used to wait forever);
 *   - with GPU rescale (an encoder-only mix, freed under mixes_mutex when its last encoder goes)
 *     two recordings run and stop, each on a thread of its own, and shutdown does not hang;
 *   - obs#29: the canvas mix's encode thread runs from obs_reset_video on, recording 1 encodes on
 *     it and its start takes no graphics (the first recording after an OBS start stalled ~49 ms);
 *   - obs#31: an encode error stops the recording and the next recording encodes again, on the
 *     canvas mix and on a GPU-rescale mix (the error path used to wait on the encode thread from
 *     the encode thread itself, a hang); an error stop queued behind a user stop and restart
 *     leaves the new recording alone.
 *
 *   gpu-encode-test <libobs data dir>   (the SDK's share/obs/libobs/, which holds the effects)
 *
 * Exit 0 = every check held. */

#include <obs.h>
#include <util/base.h>
#include <util/platform.h>
#include <util/threading.h>

#include <dirent.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

static int failures;

static void expect(bool ok, const char *what)
{
	fprintf(stderr, "%s: %s\n", ok ? "OK" : "FAIL", what);
	if (!ok)
		failures++;
}

/* obs#31: libobs log lines the encode-error checks wait for */
static volatile long errors_logged;
static volatile long stale_stops_logged;

static void log_line(int level, const char *fmt, va_list args, void *param)
{
	(void)param;
	char line[4096];
	vsnprintf(line, sizeof(line), fmt, args);
	fprintf(stderr, "[%d] %s\n", level, line);
	if (strstr(line, "Error encoding with encoder"))
		os_atomic_inc_long(&errors_logged);
	if (strstr(line, "was stopped or restarted before its encode-error stop ran"))
		os_atomic_inc_long(&stale_stops_logged);
}

/* ---- what the encoder and the output saw, per recording ---- */

static pthread_mutex_t seen_mutex = PTHREAD_MUTEX_INITIALIZER;
/* the kernel thread id: a pthread_t value is reused by the next thread on the same stack */
static long encode_thread;
static bool encode_thread_set;
static long frames_encoded;
static long packets_out;
static int64_t first_packet_sys_dts_usec;
/* obs#31: from this frame of a recording on, the encoder fails (-1 = never) */
static long fail_from_frame = -1;

static void reset_seen(void)
{
	pthread_mutex_lock(&seen_mutex);
	encode_thread_set = false;
	frames_encoded = 0;
	packets_out = 0;
	first_packet_sys_dts_usec = -1;
	pthread_mutex_unlock(&seen_mutex);
}

static long seen_packets(void)
{
	pthread_mutex_lock(&seen_mutex);
	long n = packets_out;
	pthread_mutex_unlock(&seen_mutex);
	return n;
}

static bool wait_packets(long n, int ms)
{
	for (int waited = 0; waited < ms; waited += 10) {
		if (seen_packets() >= n)
			return true;
		os_sleep_ms(10);
	}
	return false;
}

/* ---- the texture encoder ---- */

static uint8_t packet_bytes[16];

static const char *tex_get_name(void *unused)
{
	(void)unused;
	return "test texture encoder";
}

static void *tex_create(obs_data_t *settings, obs_encoder_t *encoder)
{
	(void)settings;
	return encoder;
}

static void tex_destroy(void *data)
{
	(void)data;
}

static bool tex_encode(void *data, struct encoder_frame *frame, struct encoder_packet *packet,
		       bool *received_packet)
{
	(void)data;
	(void)frame;
	(void)packet;
	*received_packet = false;
	return false;
}

static bool tex_encode_texture2(void *data, struct encoder_texture *texture, int64_t pts, uint64_t lock_key,
				uint64_t *next_key, struct encoder_packet *packet, bool *received_packet)
{
	(void)data;
	(void)texture;
	pthread_mutex_lock(&seen_mutex);
	if (!encode_thread_set) {
		encode_thread = (long)syscall(SYS_gettid);
		encode_thread_set = true;
	}
	bool fail = fail_from_frame >= 0 && frames_encoded >= fail_from_frame;
	frames_encoded++;
	pthread_mutex_unlock(&seen_mutex);
	if (fail) {
		*received_packet = false;
		return false;
	}

	packet->data = packet_bytes;
	packet->size = sizeof(packet_bytes);
	packet->pts = pts;
	packet->dts = pts;
	packet->keyframe = true;
	packet->type = OBS_ENCODER_VIDEO;
	*received_packet = true;
	*next_key = lock_key;
	return true;
}

static struct obs_encoder_info tex_encoder = {
	.id = "test_texture_encoder",
	.type = OBS_ENCODER_VIDEO,
	.codec = "h264",
	.caps = OBS_ENCODER_CAP_PASS_TEXTURE,
	.get_name = tex_get_name,
	.create = tex_create,
	.destroy = tex_destroy,
	.encode = tex_encode,
	.encode_texture2 = tex_encode_texture2,
};

/* ---- the output ---- */

static const char *out_get_name(void *unused)
{
	(void)unused;
	return "test output";
}

static void *out_create(obs_data_t *settings, obs_output_t *output)
{
	(void)settings;
	return output;
}

static void out_destroy(void *data)
{
	(void)data;
}

static bool out_start(void *data)
{
	obs_output_t *output = data;
	if (!obs_output_can_begin_data_capture(output, 0))
		return false;
	if (!obs_output_initialize_encoders(output, 0))
		return false;
	return obs_output_begin_data_capture(output, 0);
}

static void out_stop(void *data, uint64_t ts)
{
	(void)ts;
	obs_output_end_data_capture(data);
}

static void out_encoded_packet(void *data, struct encoder_packet *packet)
{
	(void)data;
	if (!packet)
		return;
	pthread_mutex_lock(&seen_mutex);
	if (first_packet_sys_dts_usec < 0)
		first_packet_sys_dts_usec = packet->sys_dts_usec;
	packets_out++;
	pthread_mutex_unlock(&seen_mutex);
}

static struct obs_output_info test_output = {
	.id = "test_texture_output",
	.flags = OBS_OUTPUT_VIDEO | OBS_OUTPUT_ENCODED,
	.get_name = out_get_name,
	.create = out_create,
	.destroy = out_destroy,
	.start = out_start,
	.stop = out_stop,
	.encoded_packet = out_encoded_packet,
};

/* "deactivate", not "stop": "stop" fires before the end-data-capture thread stops the encoders */
static os_event_t *stopped;

static void on_deactivate(void *param, calldata_t *cd)
{
	(void)param;
	(void)cd;
	os_event_signal(stopped);
}

/* ---- graphics tasks ---- */

struct task_seen {
	bool ran;
	bool on_graphics_thread;
};

static void graphics_task(void *param)
{
	struct task_seen *seen = param;
	seen->ran = true;
	seen->on_graphics_thread = obs_in_task_thread(OBS_TASK_GRAPHICS);
}

struct waiting_task {
	struct task_seen seen;
	os_event_t *done;
};

static void *queue_waiting_task(void *param)
{
	struct waiting_task *w = param;
	obs_queue_task(OBS_TASK_GRAPHICS, graphics_task, &w->seen, true);
	os_event_signal(w->done);
	return NULL;
}

/* Runs a waiting graphics task from a thread of its own; false when it did not return in `ms`. */
static bool run_waiting_task(struct task_seen *seen, unsigned long ms)
{
	/* on the heap: a thread that never returns keeps it (leaked, the test fails) */
	struct waiting_task *w = bzalloc(sizeof(*w));
	pthread_t thread;

	os_event_init(&w->done, OS_EVENT_TYPE_MANUAL);
	pthread_create(&thread, NULL, queue_waiting_task, w);
	if (os_event_timedwait(w->done, ms) != 0) {
		pthread_detach(thread);
		return false;
	}
	pthread_join(thread, NULL);
	*seen = w->seen;
	os_event_destroy(w->done);
	bfree(w);
	return true;
}

static void set_fail_from(long frame)
{
	pthread_mutex_lock(&seen_mutex);
	fail_from_frame = frame;
	pthread_mutex_unlock(&seen_mutex);
}

/* A hung encode thread also hangs obs_shutdown: report and leave without it. */
static void bail_if_hung(bool hung)
{
	if (!hung)
		return;
	fprintf(stderr, "FAIL: %d check(s) failed; the encode thread hangs, leaving without obs_shutdown\n",
		failures);
	fflush(stderr);
	_exit(1);
}

/* obs#31: a recording whose encoder fails from frame 5 on stops by itself, and the next
 * recording encodes again. */
static void encode_error_round(obs_output_t *output, const char *which)
{
	char what[256];

	reset_seen();
	set_fail_from(5);
	snprintf(what, sizeof(what), "%s recording with an encode error starts", which);
	expect(obs_output_start(output), what);
	bool stopped_by_error = os_event_timedwait(stopped, 5000) == 0;
	snprintf(what, sizeof(what), "%s recording stops by itself within 5 s after an encode error", which);
	expect(stopped_by_error, what);
	if (!stopped_by_error) {
		obs_output_stop(output);
		os_event_timedwait(stopped, 5000);
	}
	set_fail_from(-1);
	/* "deactivate" can come before the encoder's own stop on the destruction thread ends */
	obs_encoder_t *encoder = obs_output_get_video_encoder(output);
	bool inactive = false;
	for (int waited = 0; waited < 5000 && !(inactive = !obs_encoder_active(encoder)); waited += 10)
		os_sleep_ms(10);
	snprintf(what, sizeof(what), "%s encoder is inactive within 5 s after the encode error", which);
	expect(inactive, what);

	reset_seen();
	snprintf(what, sizeof(what), "%s recording after the encode error starts", which);
	expect(obs_output_start(output), what);
	bool encodes = wait_packets(10, 5000);
	snprintf(what, sizeof(what), "%s recording after the encode error sends 10 packets within 5 s", which);
	expect(encodes, what);
	obs_output_stop(output);
	bool stopped_after = os_event_timedwait(stopped, 5000) == 0;
	snprintf(what, sizeof(what), "%s recording after the encode error stops within 5 s", which);
	expect(stopped_after, what);
	bail_if_hung(!stopped_by_error || !inactive || !encodes || !stopped_after);
}

/* obs#29: the kernel TID of libobs's "obs gpu encode thread" (comm is cut to 15 bytes), 0 if none */
static long gpu_encode_tid(void)
{
	long tid = 0;
	DIR *dir = opendir("/proc/self/task");
	struct dirent *ent;

	if (!dir)
		return 0;
	while (!tid && (ent = readdir(dir))) {
		char path[300], comm[32] = {0};
		FILE *f;

		if (ent->d_name[0] == '.')
			continue;
		snprintf(path, sizeof(path), "/proc/self/task/%s/comm", ent->d_name);
		f = fopen(path, "r");
		if (!f)
			continue;
		if (fgets(comm, sizeof(comm), f) && strncmp(comm, "obs gpu encode", 14) == 0)
			tid = atol(ent->d_name);
		fclose(f);
	}
	closedir(dir);
	return tid;
}

/* obs#29: obs_output_start on a thread of its own, so this one can hold graphics meanwhile */
struct start_call {
	obs_output_t *output;
	bool ok;
	os_event_t *done;
	pthread_t thread;
};

static void *start_output_thread(void *param)
{
	struct start_call *call = param;
	call->ok = obs_output_start(call->output);
	os_event_signal(call->done);
	return NULL;
}

/* obs#31 review: the encode-error stop is queued on the destruction thread. Held there (a task
 * that waits), the user stops and restarts first; the queued stop must leave the new recording
 * alone, and the user's stop must not be repeated by it. */
static os_event_t *destroy_hold;
static os_event_t *destroy_held;

static void hold_destruction_thread(void *param)
{
	(void)param;
	os_event_signal(destroy_held);
	os_event_wait(destroy_hold);
}

static void stale_error_stop_round(obs_output_t *output)
{
	os_event_init(&destroy_hold, OS_EVENT_TYPE_MANUAL);
	os_event_init(&destroy_held, OS_EVENT_TYPE_MANUAL);
	obs_queue_task(OBS_TASK_DESTROY, hold_destruction_thread, NULL, false);
	expect(os_event_timedwait(destroy_held, 3000) == 0, "the destruction thread is held");

	reset_seen();
	long errors_before = os_atomic_load_long(&errors_logged);
	long stale_before = os_atomic_load_long(&stale_stops_logged);
	set_fail_from(5);
	expect(obs_output_start(output), "a recording whose error stop is held starts");
	/* the error is logged right before its stop is queued, on the encode thread */
	bool failed = false;
	for (int waited = 0; waited < 5000 && !failed; waited += 10) {
		failed = os_atomic_load_long(&errors_logged) > errors_before;
		os_sleep_ms(10);
	}
	expect(failed, "its encoder failed and its stop was queued");
	os_sleep_ms(100);
	set_fail_from(-1);
	obs_output_stop(output);
	expect(os_event_timedwait(stopped, 5000) == 0, "the user's stop ends it while the error stop is held");

	reset_seen();
	expect(obs_output_start(output), "the next recording starts while the old error stop is held");
	expect(wait_packets(5, 5000), "the next recording sends packets");

	os_event_signal(destroy_hold);
	bool stale_ran = false;
	for (int waited = 0; waited < 5000 && !stale_ran; waited += 10) {
		stale_ran = os_atomic_load_long(&stale_stops_logged) > stale_before;
		os_sleep_ms(10);
	}
	expect(stale_ran && os_atomic_load_long(&stale_stops_logged) == stale_before + 1,
	       "the queued error stop ran within 5 s and stopped nothing (it was for the earlier recording)");
	long before = seen_packets();
	bool still = wait_packets(before + 10, 5000);
	expect(still, "the next recording keeps encoding after the stale error stop ran");
	expect(obs_output_active(output), "the next recording is still active after the stale error stop ran");

	obs_output_stop(output);
	bool stopped_ok = os_event_timedwait(stopped, 5000) == 0;
	expect(stopped_ok, "the next recording stops within 5 s");
	os_event_destroy(destroy_hold);
	os_event_destroy(destroy_held);
	bail_if_hung(!still || !stopped_ok);
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

	/* No graphics thread yet: the task runs on its caller. */
	struct task_seen before = {0};
	bool returned = run_waiting_task(&before, 3000);
	expect(returned, "a waiting graphics task returns with no graphics thread");
	expect(returned && before.ran && !before.on_graphics_thread,
	       "with no graphics thread it ran on its caller");
	if (!returned) {
		fprintf(stderr, "%d check(s) failed\n", failures);
		return 1;
	}

	struct obs_video_info ovi = {
		.graphics_module = "libobs-opengl",
		.fps_num = 30,
		.fps_den = 1,
		.base_width = 64,
		.base_height = 64,
		.output_width = 64,
		.output_height = 64,
		.output_format = VIDEO_FORMAT_NV12,
		.adapter = 0,
		.gpu_conversion = true,
		.colorspace = VIDEO_CS_709,
		.range = VIDEO_RANGE_PARTIAL,
		.scale_type = OBS_SCALE_BICUBIC,
	};
	int reset = obs_reset_video(&ovi);
	expect(reset == OBS_VIDEO_SUCCESS, "obs_reset_video 64x64 NV12 on libobs-opengl");
	if (reset != OBS_VIDEO_SUCCESS) {
		obs_shutdown();
		fprintf(stderr, "%d check(s) failed\n", failures);
		return 1;
	}

	struct task_seen running = {0};
	expect(run_waiting_task(&running, 3000) && running.ran && running.on_graphics_thread,
	       "a waiting graphics task runs on the graphics thread");

	obs_register_encoder(&tex_encoder);
	obs_register_output(&test_output);

	obs_encoder_t *encoder = obs_video_encoder_create("test_texture_encoder", "tex", NULL, NULL);
	obs_output_t *output = obs_output_create("test_texture_output", "rec", NULL, NULL);
	expect(encoder && output, "the encoder and the output are created");
	obs_encoder_set_video(encoder, obs_get_video());
	obs_output_set_video_encoder(output, encoder);

	os_event_init(&stopped, OS_EVENT_TYPE_AUTO);
	signal_handler_connect(obs_output_get_signal_handler(output), "deactivate", on_deactivate, NULL);

	/* obs#29: the canvas mix runs its encode thread before any recording. */
	long early_thread = gpu_encode_tid();
	expect(early_thread != 0, "the canvas mix's encode thread runs before any recording (obs#29)");

	/* Recording 1: its start takes no graphics (it completes while this thread holds them). */
	reset_seen();
	struct start_call call = {.output = output};
	os_event_init(&call.done, OS_EVENT_TYPE_MANUAL);
	obs_enter_graphics();
	pthread_create(&call.thread, NULL, start_output_thread, &call);
	bool started_held = os_event_timedwait(call.done, 3000) == 0;
	obs_leave_graphics();
	pthread_join(call.thread, NULL);
	os_event_destroy(call.done);
	expect(started_held, "recording 1 starts within 3 s while another thread holds graphics (obs#29)");
	expect(call.ok, "recording 1 starts");
	expect(wait_packets(10, 5000), "recording 1 sends 10 packets within 5 s");
	pthread_mutex_lock(&seen_mutex);
	long first_thread = encode_thread;
	bool first_set = encode_thread_set;
	pthread_mutex_unlock(&seen_mutex);
	expect(first_set && first_thread == early_thread,
	       "recording 1 encodes on the encode thread that ran before it (obs#29)");
	obs_output_stop(output);
	expect(os_event_timedwait(stopped, 5000) == 0, "recording 1's encoders stop within 5 s");
	uint64_t stopped_1_ns = os_gettime_ns();

	os_sleep_ms(500);

	/* Recording 2. */
	reset_seen();
	expect(obs_output_start(output), "recording 2 starts");
	expect(wait_packets(10, 5000), "recording 2 sends 10 packets within 5 s");
	pthread_mutex_lock(&seen_mutex);
	long second_thread = encode_thread;
	bool second_set = encode_thread_set;
	int64_t first_dts_usec = first_packet_sys_dts_usec;
	pthread_mutex_unlock(&seen_mutex);

	expect(first_set && second_set && first_thread == second_thread,
	       "both recordings encode on the same thread");
	/* A frame left from recording 1 is older than its stop; the 500 ms gap leaves room for
	 * a new frame's tick time. */
	fprintf(stderr, "recording 2 first packet %lld us, recording 1 stopped at %llu us\n", (long long)first_dts_usec,
		(unsigned long long)(stopped_1_ns / 1000));
	expect(first_dts_usec > 0 && (uint64_t)first_dts_usec * 1000 > stopped_1_ns + 250000000ULL,
	       "recording 2 starts with a new frame, not one from recording 1");

	/* The stop takes no graphics: it completes while this thread holds the graphics lock. */
	obs_enter_graphics();
	obs_output_stop(output);
	bool stopped_held = os_event_timedwait(stopped, 3000) == 0;
	obs_leave_graphics();
	expect(stopped_held, "recording 2's encoders stop within 3 s while another thread holds graphics");
	if (!stopped_held)
		os_event_timedwait(stopped, 5000);

	encode_error_round(output, "a canvas-mix");
	stale_error_stop_round(output);

	/* GPU rescale: the encoder gets a mix of its own, freed when it stops. */
	obs_encoder_set_scaled_size(encoder, 32, 32);
	obs_encoder_set_gpu_scale_type(encoder, OBS_SCALE_BICUBIC);
	long scaled_threads[2] = {0, 0};
	for (int r = 0; r < 2; r++) {
		reset_seen();
		expect(obs_output_start(output), "a GPU-rescaled recording starts");
		expect(wait_packets(10, 5000), "the GPU-rescaled recording sends 10 packets within 5 s");
		pthread_mutex_lock(&seen_mutex);
		scaled_threads[r] = encode_thread_set ? encode_thread : 0;
		pthread_mutex_unlock(&seen_mutex);
		obs_output_stop(output);
		expect(os_event_timedwait(stopped, 5000) == 0, "the GPU-rescaled recording's encoders stop within 5 s");
	}
	expect(scaled_threads[0] && scaled_threads[1] && scaled_threads[0] != second_thread,
	       "a GPU-rescaled recording encodes on its own mix's thread");

	encode_error_round(output, "a GPU-rescaled");

	obs_output_release(output);
	obs_encoder_release(encoder);
	obs_shutdown();
	os_event_destroy(stopped);

	fprintf(stderr, "%s: %d check(s) failed\n", failures ? "FAIL" : "PASS", failures);
	return failures ? 1 : 0;
}
