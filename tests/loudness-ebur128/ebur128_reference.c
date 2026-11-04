/* camera-box#1436: the loudness the stream operator reads off obs-loudness-dock comes from the
 * vendored libebur128 (vendor/obs-loudness-dock/deps/libebur128). This feeds it EBU Tech 3341
 * test case 1 -- a 1 kHz sine at -23 dBFS on both channels of a 48 kHz stereo signal -- the way
 * the dock does (its LOUDNESS_DOCK_EBUR128_MODE, float frames in blocks), and checks
 * the meter reads -23 LUFS momentary, short-term and integrated, and the true peak near -23 dBTP.
 * A silent signal must read no loudness (-inf), so a meter stuck at a value fails too.
 *
 * The dock reads integrated loudness and LRA every 48 ms under the mutex its audio callback takes
 * on OBS's audio thread. Without EBUR128_MODE_HISTOGRAM libebur128 walks every 100 ms block since
 * start, so the hold grows with uptime (deep review of v0.4.12: ~3 ms at 8 h, ~9 ms at 24 h,
 * against a 21.3 ms audio tick). The test meters with the dock's own mode (src/loudness-mode.h),
 * checks it has histogram mode, and that the reference still reads -23 LUFS and LRA 0 in it. */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#include "ebur128.h"
#include "loudness-mode.h"

#define RATE 48000
#define CHANNELS 2
#define BLOCK 1024

static int failures;

static void expect_near(const char *what, double got, double want, double tol)
{
	int ok = fabs(got - want) <= tol;
	printf("%s %s: %.3f (want %.1f +- %.1f)\n", ok ? "ok  " : "FAIL", what, got, want, tol);
	if (!ok)
		failures++;
}

static ebur128_state *meter(void)
{
	ebur128_state *st = ebur128_init(CHANNELS, RATE, LOUDNESS_DOCK_EBUR128_MODE);
	if (!st) {
		fprintf(stderr, "FAIL ebur128_init\n");
		exit(1);
	}
	return st;
}

/* Feeds `seconds` of a 1 kHz sine at `dbfs` (0 amplitude when dbfs is -INFINITY). */
static void feed_sine(ebur128_state *st, double dbfs, int seconds)
{
	const double amp = isinf(dbfs) ? 0.0 : pow(10.0, dbfs / 20.0);
	float buf[BLOCK * CHANNELS];
	long n = 0;
	for (long left = (long)RATE * seconds; left > 0; left -= BLOCK) {
		size_t frames = left < BLOCK ? (size_t)left : BLOCK;
		for (size_t i = 0; i < frames; i++, n++) {
			float s = (float)(amp * sin(2.0 * M_PI * 1000.0 * (double)n / RATE));
			buf[i * CHANNELS] = s;
			buf[i * CHANNELS + 1] = s;
		}
		if (ebur128_add_frames_float(st, buf, frames) != EBUR128_SUCCESS) {
			fprintf(stderr, "FAIL ebur128_add_frames_float\n");
			exit(1);
		}
	}
}

int main(void)
{

	ebur128_state *st = meter();
	feed_sine(st, -23.0, 20);
	int histogram = (LOUDNESS_DOCK_EBUR128_MODE & EBUR128_MODE_HISTOGRAM) == EBUR128_MODE_HISTOGRAM;
	printf("%s dock meter mode has EBUR128_MODE_HISTOGRAM\n", histogram ? "ok  " : "FAIL");
	if (!histogram)
		failures++;
	double m, s, i, lra, peak0, peak1;
	if (ebur128_loudness_momentary(st, &m) || ebur128_loudness_shortterm(st, &s) ||
	    ebur128_loudness_global(st, &i) || ebur128_loudness_range(st, &lra) ||
	    ebur128_true_peak(st, 0, &peak0) ||
	    ebur128_true_peak(st, 1, &peak1)) {
		fprintf(stderr, "FAIL a loudness read returned an error\n");
		return 1;
	}
	expect_near("momentary LUFS", m, -23.0, 0.1);
	expect_near("short-term LUFS", s, -23.0, 0.1);
	expect_near("integrated LUFS", i, -23.0, 0.1);
	expect_near("loudness range LU", lra, 0.0, 0.1);
	expect_near("true peak L dBTP", 20.0 * log10(peak0), -23.0, 0.5);
	expect_near("true peak R dBTP", 20.0 * log10(peak1), -23.0, 0.5);
	ebur128_destroy(&st);

	st = meter();
	feed_sine(st, -INFINITY, 5);
	if (ebur128_loudness_global(st, &i)) {
		fprintf(stderr, "FAIL ebur128_loudness_global on silence\n");
		return 1;
	}
	int silent = isinf(i) && i < 0;
	printf("%s silence integrated: %f (want -inf)\n", silent ? "ok  " : "FAIL", i);
	if (!silent)
		failures++;
	ebur128_destroy(&st);

	printf("%s\n", failures ? "loudness-ebur128: FAILED" : "loudness-ebur128: all checks passed");
	return failures ? 1 : 0;
}
