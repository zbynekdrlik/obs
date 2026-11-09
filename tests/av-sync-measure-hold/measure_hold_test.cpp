/* camera-box issue 1495 (stream-snv 9.11.2026): the A/V-sync dock looked `mbc` up every 5 s and kept
 * the reference. A removed `mbc` was still returned by obs_get_source_by_name, so the dock re-pinned
 * it and it was never destroyed: still in GetInputList, its name refused by CreateInput until OBS
 * restarted. This runs the dock's hold (camera-box-measure-hold.hpp) through real libobs. */
#include <obs.h>
#include <util/platform.h>

#include <atomic>
#include <cstdio>
#include <cstring>
#include <mutex>

#include "camera-box-measure-hold.hpp"

static int failures;

static void check(bool ok, const char *what)
{
	std::printf("%s %s\n", ok ? "ok  " : "FAIL", what);
	if (!ok)
		failures++;
}

static const char *test_get_name(void *)
{
	return "measure-hold test source";
}

static void *test_create(obs_data_t *, obs_source_t *)
{
	static int dummy;
	return &dummy;
}

static void test_destroy(void *) {}

/* The dock's hold: the mutex and the reference its decode worker keeps for the audio thread. */
struct Hold {
	std::mutex mutex;
	obs_source_t *src = nullptr;
	int dropped = 0;
};

/* What the dock connects to libobs's global "source_remove" signal. */
static void on_source_remove(void *param, calldata_t *cd)
{
	auto *h = static_cast<Hold *>(param);
	auto *removed = static_cast<obs_source_t *>(calldata_ptr(cd, "source"));
	if (camerabox::cb_measure_hold_drop_removed(h->mutex, h->src, removed))
		h->dropped++;
}

static void on_destroy(void *param, calldata_t *)
{
	static_cast<std::atomic<int> *>(param)->fetch_add(1);
}

/* libobs destroys a released source on its own destruction thread (obs_source_destroy_defer), and
 * obs_wait_for_destroy_queue is a no-op without video and audio threads: wait for the "destroy"
 * signal, at most 5 s. */
static bool destroyed_within_5s(const std::atomic<int> *destroyed)
{
	for (int i = 0; i < 500; i++) {
		if (destroyed->load() == 1)
			return true;
		os_sleep_ms(10);
	}
	return false;
}

/* The dock's refresh: look the name up, a removed source counts as absent, keep the result. */
static void refresh(Hold &h, const char *name)
{
	obs_source_t *src = camerabox::cb_measure_source_if_live(obs_get_source_by_name(name));
	obs_source_t *old;
	{
		std::lock_guard<std::mutex> lock(h.mutex);
		old = h.src;
		h.src = src;
	}
	obs_source_release(old);
}

static obs_source_t *create_mbc(std::atomic<int> *destroyed)
{
	obs_source_t *src = obs_source_create("cb_measure_hold_test", "mbc", nullptr, nullptr);
	signal_handler_connect(obs_source_get_signal_handler(src), "destroy", on_destroy, destroyed);
	return src;
}

int main()
{
	if (!obs_startup("en-US", nullptr, nullptr)) {
		std::fprintf(stderr, "FAIL: obs_startup\n");
		return 1;
	}

	struct obs_source_info info = {};
	info.id = "cb_measure_hold_test";
	info.type = OBS_SOURCE_TYPE_INPUT;
	info.get_name = test_get_name;
	info.create = test_create;
	info.destroy = test_destroy;
	obs_register_source(&info);

	Hold h;
	signal_handler_connect(obs_get_signal_handler(), "source_remove", on_source_remove, &h);

	/* 1. The dock holds `mbc`; the operator removes it: the dock lets go at once, it is destroyed. */
	std::atomic<int> destroyed1{0};
	obs_source_t *mbc = create_mbc(&destroyed1);
	check(mbc != nullptr, "mbc created");
	refresh(h, "mbc");
	check(h.src == mbc, "the dock holds mbc");
	obs_source_remove(mbc);
	check(h.src == nullptr && h.dropped == 1, "removing mbc drops the dock's reference at once");
	obs_source_release(mbc);
	check(destroyed_within_5s(&destroyed1), "the removed mbc is destroyed");
	obs_source_t *gone = obs_get_source_by_name("mbc");
	check(gone == nullptr, "the removed mbc is no longer listed");
	obs_source_release(gone);

	/* 2. A removed mbc still alive through another holder: the dock's refresh says absent and holds
	 * nothing, and a new mbc can be created and is the one found. */
	std::atomic<int> destroyed2{0};
	obs_source_t *old_mbc = create_mbc(&destroyed2);
	obs_source_t *other_holder = obs_source_get_ref(old_mbc);
	obs_source_remove(old_mbc);
	obs_source_release(old_mbc);
	refresh(h, "mbc");
	check(h.src == nullptr, "a removed mbc is absent to the dock's refresh");
	obs_source_release(other_holder);
	check(destroyed_within_5s(&destroyed2), "the removed mbc is destroyed when its last other holder lets go");

	std::atomic<int> destroyed3{0};
	obs_source_t *new_mbc = create_mbc(&destroyed3);
	refresh(h, "mbc");
	check(h.src == new_mbc, "a new mbc is found and held");

	/* 3. Another source removed: the dock's reference stays. */
	obs_source_t *other = obs_source_create("cb_measure_hold_test", "other", nullptr, nullptr);
	obs_source_remove(other);
	obs_source_release(other);
	check(h.src == new_mbc && h.dropped == 1, "removing another source keeps the dock's reference");

	refresh(h, "nosuch");
	check(h.src == nullptr, "a missing name holds nothing");
	obs_source_remove(new_mbc);
	obs_source_release(new_mbc);
	check(destroyed_within_5s(&destroyed3), "the new mbc is destroyed after removal");

	signal_handler_disconnect(obs_get_signal_handler(), "source_remove", on_source_remove, &h);
	obs_shutdown();
	std::printf("%s: %d failure(s)\n", failures ? "FAIL" : "PASS", failures);
	return failures ? 1 : 0;
}
