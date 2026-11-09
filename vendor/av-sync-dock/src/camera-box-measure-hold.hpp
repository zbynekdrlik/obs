#pragma once

/*
 * camera-box A/V-sync dock -- the measurement source reference the dock holds (camera-box issue 1495).
 *
 * The decode worker looks CAMERA_BOX_MEASURE_SOURCE_NAME up (cb_refresh_measure_source) and keeps a
 * strong reference for the mix-tick callback (issue 1470). libobs keeps a removed source in its
 * public list until it is destroyed, so the lookup must treat a removed source as absent, and the
 * held reference must go when the source is removed: otherwise the removed `mbc` is never destroyed,
 * stays in GetInputList and CreateInput refuses its name until OBS restarts (stream-snv 9.11.2026).
 *
 * libobs only, no dock state: tests/av-sync-measure-hold runs these through real libobs.
 */

#include <obs.h>

#include <mutex>

namespace camerabox {

/* A lookup's result (a strong reference or nullptr), with a removed source counted as absent. */
inline obs_source_t *cb_measure_source_if_live(obs_source_t *src)
{
	if (src && obs_source_removed(src)) {
		obs_source_release(src);
		return nullptr;
	}
	return src;
}

/* The decode worker's swap: hold `src` (a strong reference or nullptr) and release the old one after
 * the lock, so a last release never runs while the audio thread waits on it (issue 1470).
 * camera-box issue 1495: a source removed since its lookup is not held. obs_source_remove sets the
 * flag before its signal, whose handler takes this lock: either the handler finds `src` held and
 * drops it, or it ran first and this check, under the same lock, sees the flag. */
inline void cb_measure_hold_swap(std::mutex &mutex, obs_source_t *&held, obs_source_t *src)
{
	obs_source_t *old;
	obs_source_t *removed = nullptr;
	{
		std::lock_guard<std::mutex> lock(mutex);
		old = held;
		if (src && obs_source_removed(src)) {
			removed = src;
			src = nullptr;
		}
		held = src;
	}
	obs_source_release(old);
	obs_source_release(removed);
}

/* libobs's global "source_remove" signal: when `removed` is the held source, drop and release the
 * held reference. Returns whether it was dropped. The release is after the lock (the audio thread
 * reads the source under it), and never the last one: obs_source_remove holds its own reference
 * across the signal. Runs on the thread that removes the source. */
inline bool cb_measure_hold_drop_removed(std::mutex &mutex, obs_source_t *&held, obs_source_t *removed)
{
	if (!removed)
		return false;
	{
		std::lock_guard<std::mutex> lock(mutex);
		if (held != removed)
			return false;
		held = nullptr;
	}
	obs_source_release(removed);
	return true;
}

} // namespace camerabox
