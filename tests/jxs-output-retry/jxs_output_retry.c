/* camera-box#1387 (strih-snv reboot 4.11.2026 20:49): OBS started before the network, the JPEG XS
 * program output's sender could not connect ("Network is unreachable"), the output logged "did not
 * start" and was never tried again -- the stream program stayed black for 30 min. The output's
 * supervisor now retries a wanted, inactive output every 2 s and logs a start error once per outage.
 * This runs those pure decisions (jxs-output-decide.h). */
#include <stdio.h>
#include <string.h>

#include "jxs-output-decide.h"

static int failures;

#define CHECK(cond, what)                                              \
	do {                                                           \
		int ok_ = (cond);                                      \
		printf("%s %s\n", ok_ ? "ok  " : "FAIL", what);        \
		if (!ok_)                                              \
			failures++;                                    \
	} while (0)

#define S 1000000000ULL

int main(void)
{
	struct cb_jxs_out_retry r = {0};

	/* the reboot: wanted (enabled=true), the first start failed at t=10 s */
	cb_jxs_out_retry_note(&r, false, 10 * S);
	CHECK(r.fails == 1, "a failed start is counted");
	CHECK(!cb_jxs_out_retry_due(&r, true, false, 10 * S + S), "no retry before 2 s");
	CHECK(cb_jxs_out_retry_due(&r, true, false, 12 * S), "a wanted, inactive output retries 2 s later");
	CHECK(CB_JXS_OUT_RETRY_NS == 2 * S, "the retry period is 2 s");

	/* the network is still down: each failure pushes the next try 2 s on */
	cb_jxs_out_retry_note(&r, false, 12 * S);
	CHECK(r.fails == 2, "a second failure is counted");
	CHECK(!cb_jxs_out_retry_due(&r, true, false, 13 * S), "the next try waits its 2 s");
	CHECK(cb_jxs_out_retry_due(&r, true, false, 14 * S), "and then comes");

	/* never a start while it runs, or after StopOutput (no longer wanted) */
	CHECK(!cb_jxs_out_retry_due(&r, true, true, 100 * S), "an active output is never started");
	CHECK(!cb_jxs_out_retry_due(&r, false, false, 100 * S), "an unwanted (stopped) output is never started");

	/* the network came up: the start succeeds, the state resets */
	cb_jxs_out_retry_note(&r, true, 14 * S);
	CHECK(r.fails == 0 && r.next_ns == 0, "a success resets the retry");
	CHECK(cb_jxs_out_retry_due(&r, true, false, 14 * S), "a reset retry starts at once when it is wanted and down");

	/* the log: one line per distinct error, an outage of the same error logs once */
	char last[CB_JXS_OUT_ERROR_LEN] = "";
	CHECK(cb_jxs_out_error_new(last, sizeof last, "10.77.9.204:7100: Network is unreachable (os error 101)"),
	      "the first error is logged");
	CHECK(!cb_jxs_out_error_new(last, sizeof last, "10.77.9.204:7100: Network is unreachable (os error 101)"),
	      "the same error again is not");
	CHECK(cb_jxs_out_error_new(last, sizeof last, "10.77.9.204:7100: No route to host (os error 113)"),
	      "a different error is logged");
	cb_jxs_out_error_clear(last);
	CHECK(cb_jxs_out_error_new(last, sizeof last, "10.77.9.204:7100: No route to host (os error 113)"),
	      "after a success the next outage is logged again");
	char small[8] = "";
	CHECK(cb_jxs_out_error_new(small, sizeof small, "a long error text") && strlen(small) < sizeof small,
	      "a long error is kept truncated, never past its buffer");
	CHECK(!cb_jxs_out_error_new(small, sizeof small, "a long error text"), "and the same long error is not logged twice");

	/* a LIVE sender that died (deep review of v0.4.14): a panic poisons it (every frame -2), a
	 * panicked send thread or feeder sends no frame whole while frames are handed to it. A send
	 * error is no death: vstack counts a frame with failed sends as sent whole (send_errors only),
	 * so a receiver that is off restarts nothing. */
	uint32_t dead = 0;
	CHECK(!cb_jxs_out_window_dead(&dead, 150, 150, false), "a window that sent frames is alive");
	CHECK(!cb_jxs_out_window_dead(&dead, 0, 0, false), "a window with nothing handed to the sender is no death");
	CHECK(!cb_jxs_out_window_dead(&dead, 150, 0, false), "one window with nothing sent is not yet a death");
	CHECK(cb_jxs_out_window_dead(&dead, 150, 0, false), "two windows in a row with nothing sent is");
	dead = 0;
	CHECK(!cb_jxs_out_window_dead(&dead, 150, 0, false), "a fresh count after a restart");
	CHECK(!cb_jxs_out_window_dead(&dead, 150, 3, false), "a window that sent again resets it");
	CHECK(!cb_jxs_out_window_dead(&dead, 150, 0, false), "so a later stall counts from one again");
	dead = 0;
	CHECK(cb_jxs_out_window_dead(&dead, 1, 1, true), "a poisoned sender is dead at once");
	CHECK(CB_JXS_OUT_DEAD_WINDOWS == 2, "two 5 s windows (10 s) without a frame sent restart the sender");

	/* a sender whose threads are gone hands out no status window at all */
	CHECK(!cb_jxs_out_stats_stale(100 * S, 114 * S, 50), "no window for 14 s is not yet stale");
	CHECK(cb_jxs_out_stats_stale(100 * S, 115 * S, 50), "no window for 15 s while frames are handed to the sender is");
	CHECK(!cb_jxs_out_stats_stale(100 * S, 200 * S, 0), "nothing handed is never stale");
	CHECK(!cb_jxs_out_stats_stale(0, 200 * S, 50), "no start time yet is never stale");

	/* the supervisor's restart of a dead sender is no failed try */
	struct cb_jxs_out_retry rs = {0};
	cb_jxs_out_retry_restart(&rs, 50 * S);
	CHECK(rs.fails == 0, "a restart counts no failed try");
	CHECK(!cb_jxs_out_retry_due(&rs, true, false, 51 * S) && cb_jxs_out_retry_due(&rs, true, false, 52 * S),
	      "and starts the sender again 2 s later");

	printf("%s\n", failures ? "jxs-output-retry: FAILED" : "jxs-output-retry: all checks passed");
	return failures ? 1 : 0;
}
