/* camera-box#1387 (obs#1): obs-websocket's GL work runs on the graphics thread.
 *
 * obs-websocket runs every request on a Qt thread pool. GetSourceScreenshot and CreateSceneItem
 * entered graphics from that thread. A pool thread idle for 30 s ends, so a request every 5 min
 * (camera-box's program watchdog) always ran on a fresh thread. Its first GL MakeCurrent took
 * about 60 ms under the graphics lock, and the render tick lost a JPEG XS frame. The test builds
 * the plugin's libobs-only helper (Obs_GraphicsTask.cpp) against real libobs and libobs-opengl.
 * It checks:
 *
 *   - a screenshot render runs on the graphics thread, not on the requesting thread, and its
 *     pixels come back;
 *   - a caller that already holds graphics gets its screenshot (no wait on itself);
 *   - a scene item add runs on the graphics thread and returns a referenced item.
 *
 *   ws-graphics-test <libobs data dir>   (the SDK's share/obs/libobs/, which holds the effects)
 *
 * Exit 0 = every check held. */

#include <obs.h>
#include <graphics/graphics.h>
#include <graphics/vec4.h>
#include <util/base.h>
#include <util/platform.h>
#include <util/threading.h>

#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <vector>

#include "Obs_GraphicsTask.h"

static int failures;

static void expect(bool ok, const char *what)
{
	fprintf(stderr, "%s: %s\n", ok ? "OK" : "FAIL", what);
	if (!ok)
		failures++;
}

static void log_line(int level, const char *fmt, va_list args, void *)
{
	char line[4096];
	vsnprintf(line, sizeof(line), fmt, args);
	fprintf(stderr, "[%d] %s\n", level, line);
}

/* the kernel thread id: a pthread_t value is reused by the next thread on the same stack */
static long tid(void)
{
	return (long)syscall(SYS_gettid);
}

/* ---- a 16x16 source that paints red and records the thread it rendered on ---- */

static const uint32_t kSize = 16;
static volatile long render_tid;

static const char *red_get_name(void *)
{
	return "test red";
}

static void *red_create(obs_data_t *, obs_source_t *)
{
	return bzalloc(1);
}

static void red_destroy(void *data)
{
	bfree(data);
}

static uint32_t red_get_size(void *)
{
	return kSize;
}

static void red_render(void *, gs_effect_t *)
{
	render_tid = tid();
	vec4 red;
	vec4_set(&red, 1.0f, 0.0f, 0.0f, 1.0f);
	gs_clear(GS_CLEAR_COLOR, &red, 0.0f, 0);
}

/* ---- the graphics thread's id ---- */

static void record_tid(void *param)
{
	*static_cast<long *>(param) = tid();
}

/* ---- a call that may hang runs on a thread of its own, joined with a timeout ---- */

struct Call {
	void (*fn)(Call *);
	obs_source_t *source;
	obs_scene_t *scene;
	std::vector<uint8_t> pixels;
	bool success = false;
	long caller = 0;
	obs_sceneitem_t *item = nullptr;
	os_event_t *done = nullptr;
};

static void *call_thread(void *param)
{
	Call *c = static_cast<Call *>(param);
	c->caller = tid();
	c->fn(c);
	os_event_signal(c->done);
	return nullptr;
}

/* false when it did not return in `ms` (the Call is leaked, the test fails) */
static bool run_call(Call *c, unsigned long ms)
{
	pthread_t thread;
	os_event_init(&c->done, OS_EVENT_TYPE_MANUAL);
	pthread_create(&thread, nullptr, call_thread, c);
	if (os_event_timedwait(c->done, ms) != 0) {
		pthread_detach(thread);
		return false;
	}
	pthread_join(thread, nullptr);
	os_event_destroy(c->done);
	return true;
}

static void screenshot(Call *c)
{
	c->pixels.assign(kSize * kSize * 4, 0);
	c->success = Utils::Obs::GraphicsTask::RenderSourceRGBA(c->source, kSize, kSize, c->pixels.data(), kSize * 4);
}

static void screenshot_holding_graphics(Call *c)
{
	obs_enter_graphics();
	screenshot(c);
	obs_leave_graphics();
}

static bool all_red(const std::vector<uint8_t> &p)
{
	if (p.size() != kSize * kSize * 4)
		return false;
	for (size_t i = 0; i < p.size(); i += 4)
		if (p[i] != 255 || p[i + 1] != 0 || p[i + 2] != 0 || p[i + 3] != 255)
			return false;
	return true;
}

static volatile long item_add_tid;

static void on_item_add(void *, calldata_t *)
{
	item_add_tid = tid();
}

static void add_item(Call *c)
{
	c->item = Utils::Obs::GraphicsTask::AddSceneItem(c->scene, c->source, true, nullptr, nullptr);
}

int main(int argc, char **argv)
{
	if (argc != 2) {
		fprintf(stderr, "usage: %s <libobs data dir>\n", argv[0]);
		return 2;
	}
	base_set_log_handler(log_line, nullptr);

	if (!obs_startup("en-US", nullptr, nullptr)) {
		fprintf(stderr, "FAIL: obs_startup\n");
		return 1;
	}
	/* The SDK is not where libobs was configured to live: name its data dir (the effects). */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
	obs_add_data_path(argv[1]);
#pragma GCC diagnostic pop

	obs_video_info ovi = {};
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

	long graphics_tid = 0;
	obs_queue_task(OBS_TASK_GRAPHICS, record_tid, &graphics_tid, true);
	expect(graphics_tid != 0 && graphics_tid != tid(), "the graphics thread has an id of its own");

	obs_source_info red = {};
	red.id = "test_red_source";
	red.type = OBS_SOURCE_TYPE_INPUT;
	red.output_flags = OBS_SOURCE_VIDEO;
	red.get_name = red_get_name;
	red.create = red_create;
	red.destroy = red_destroy;
	red.get_width = red_get_size;
	red.get_height = red_get_size;
	red.video_render = red_render;
	obs_register_source(&red);

	obs_source_t *source = obs_source_create("test_red_source", "red", nullptr, nullptr);
	expect(source != nullptr, "test source created");

	/* GetSourceScreenshot from a request thread */
	Call *shot = new Call();
	shot->fn = screenshot;
	shot->source = source;
	render_tid = 0;
	bool returned = run_call(shot, 3000);
	expect(returned, "a screenshot returns");
	if (returned) {
		expect(shot->success && all_red(shot->pixels), "the screenshot holds the source's pixels");
		expect(render_tid == graphics_tid, "the screenshot renders on the graphics thread");
		expect(render_tid != shot->caller, "the screenshot does not render on the requesting thread");
		delete shot;
	}

	/* a caller that already holds graphics: runs where it is, no wait on the graphics thread */
	Call *held = new Call();
	held->fn = screenshot_holding_graphics;
	held->source = source;
	render_tid = 0;
	returned = run_call(held, 3000);
	expect(returned, "a screenshot from a graphics holder returns");
	if (returned) {
		expect(held->success && all_red(held->pixels), "the graphics holder's screenshot holds the pixels");
		expect(render_tid == held->caller, "a graphics holder renders on its own thread");
		delete held;
	}

	/* CreateSceneItem from a request thread */
	obs_scene_t *scene = obs_scene_create("scene");
	signal_handler_connect(obs_source_get_signal_handler(obs_scene_get_source(scene)), "item_add", on_item_add,
			       nullptr);
	Call *add = new Call();
	add->fn = add_item;
	add->source = source;
	add->scene = scene;
	item_add_tid = 0;
	returned = run_call(add, 3000);
	expect(returned, "a scene item add returns");
	if (returned) {
		expect(add->item != nullptr && obs_sceneitem_get_source(add->item) == source,
		       "the scene item add returns the item");
		expect(item_add_tid == graphics_tid, "the scene item add runs on the graphics thread");
		obs_sceneitem_release(add->item);
		delete add;
	}
	signal_handler_disconnect(obs_source_get_signal_handler(obs_scene_get_source(scene)), "item_add", on_item_add,
				  nullptr);

	obs_scene_release(scene);
	obs_source_release(source);
	obs_shutdown();

	if (failures)
		fprintf(stderr, "%d check(s) failed\n", failures);
	else
		fprintf(stderr, "PASS: 0 check(s) failed\n");
	return failures ? 1 : 0;
}
