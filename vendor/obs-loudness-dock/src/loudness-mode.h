/* camera-box#1436: the libebur128 mode the dock meters with, shared with tests/loudness-ebur128 so
 * the reference test runs the exact mode the dock does. EBUR128_MODE_HISTOGRAM keeps the integrated /
 * LRA read constant-cost (0.1 LU bins); without it the read walks every block since start under the
 * mutex audio_cb takes on OBS's audio thread, a hold that grows with uptime. */
#pragma once

#include "ebur128.h"

#define LOUDNESS_DOCK_EBUR128_MODE                                                                       \
	(EBUR128_MODE_M | EBUR128_MODE_S | EBUR128_MODE_I | EBUR128_MODE_LRA | EBUR128_MODE_TRUE_PEAK | \
	 EBUR128_MODE_HISTOGRAM)
