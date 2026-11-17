"""Tests for soak_verdict.py (obs#71). Run: python3 -m unittest discover -s plugins/dante-input/ci"""

import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import soak_verdict as sv  # noqa: E402

# Lines in the shapes run 43953607643 logged (the peer's WARN, dante-input's obs#71 line).
PEER = [
    "2026-11-16T21:10:05.648652Z  WARN dstack::peer::tone: dstack-dev peer: a tone break "
    "rx_channel=1 samples=1960 ms=20.416666666666668 gaps=2 kind=silence",
    "2026-11-16T21:10:05.648694Z  WARN dstack::peer::tone: dstack-dev peer: a tone break "
    "rx_channel=2 samples=1960 ms=20.416666666666668 gaps=2 kind=silence",
    "2026-11-16T21:11:00.000000Z  WARN dstack::peer::tone: dstack-dev peer: a tone break "
    "rx_channel=1 samples=744 ms=7.75 gaps=3 kind=silence",
    "2026-11-16T21:20:00.000000Z  WARN dstack::peer::tone: dstack-dev peer: a tone break "
    "rx_channel=1 samples=96428 ms=1004.4583333333334 gaps=4 kind=silence",
]
OBS = [
    "[200] dante-input pump: a late wake at 2026-11-16T21:10:05.600Z behind 8604 us "
    "(re-bootstrapped: blocks lost) (obs#71)",
    "[300] dante-input pump: the last minute's latest wake 8604 us behind (obs#71)",
    "[300] dante-input status: clock=media underruns=7",
]
FROM = sv.utc_ms("2026-11-16T21:00:00.000")
TO = sv.utc_ms("2026-11-16T21:15:00.000")


class SoakVerdict(unittest.TestCase):
    def test_utc_ms(self):
        self.assertEqual(sv.utc_ms("2026-11-16T21:10:05.648652"), 1_794_863_405_648)
        self.assertEqual(sv.utc_ms("1970-01-01T00:00:01.5"), 1_500)

    def test_breaks_are_deduplicated_across_channels_and_windowed(self):
        b = sv.breaks(PEER, FROM, TO)
        # the 21:20 break (the negative control's) is outside the soak's run
        self.assertEqual(
            b, [(1_794_863_405_648, 20.416666666666668), (1_794_863_460_000, 7.75)]
        )

    def test_a_break_after_a_late_wake_is_runner_one_without_is_ours(self):
        v = sv.verdict(sv.breaks(PEER, FROM, TO), sv.wakes(OBS))
        self.assertEqual(
            v[0],
            "RUNNER: engine pump descheduled 8.6 ms at 21:10:05.648 (a 20.42 ms tone break "
            "starting +28 ms after the pump's late wake, re-bootstrapped: blocks lost)",
        )
        self.assertEqual(
            v[1],
            "OURS: a 7.75 ms tone break reported at 21:11:00.000 with no pump late wake, no TX "
            "schedule restart -50..+250 ms before its start and no peer pump skip after it",
        )

    def test_the_match_window_edges(self):
        w = [(10_000, 3_000, "within the margin")]
        # the window is on the break's start: its report time less its 5 ms
        inside = sv.verdict(
            [(10_005 + sv.MATCH_AFTER_MS, 5.0), (10_005 - sv.MATCH_BEFORE_MS, 5.0)], w
        )
        self.assertTrue(all(line.startswith("RUNNER:") for line in inside), inside)
        outside = sv.verdict(
            [(10_005 + sv.MATCH_AFTER_MS + 1, 5.0), (10_005 - sv.MATCH_BEFORE_MS - 1, 5.0)], w
        )
        self.assertTrue(all(line.startswith("OURS:") for line in outside), outside)

    def test_a_long_break_is_matched_by_its_start_not_its_report(self):
        # a 1004 ms break reported 1004 ms after it began, 20 ms after a late wake
        w = [(10_000, 9_000, "re-bootstrapped: blocks lost")]
        (line,) = sv.verdict([(10_000 + 20 + 1004, 1004.4583)], w)
        self.assertTrue(line.startswith("RUNNER:"), line)
        self.assertIn("starting +20 ms", line)

    def test_the_nearest_wake_is_named(self):
        w = [(10_000, 9_000, "a"), (10_100, 3_000, "b")]
        (line,) = sv.verdict([(10_090, 5.0)], w)
        self.assertIn("descheduled 3.0 ms", line)

    LATE = (
        '[200] dante-input dlink:  WARN dstack::stream::watch: engine pump lost or overran blocks '
        'this second device="obs-ci" rebootstraps=1 missed=7 overruns=0 writes_after_send=0 '
        'max_process_us=57 media=true block=128 margin=576 playout=803 offset=1507 cause=late '
        'behind_max_us=8552 oversleep_max_us=8401 drain_max_us=80 drain_max_datagrams=34 '
        'max_process_cpu_us=49'
    )

    def test_pump_seconds_parse_the_v0_6_159_warning(self):
        (f,) = sv.pump_seconds([self.LATE, "[300] dante-input status: underruns=7"])
        self.assertEqual(f["device"], "obs-ci")
        self.assertEqual(f["cause"], "late")
        self.assertEqual(sv.num(f, "oversleep_max_us"), 8401)
        self.assertEqual(sv.num(f, "drain_max_datagrams"), 34)

    def test_a_late_wake_whose_sleep_overslept_is_runner(self):
        line = sv.split_verdict(sv.pump_seconds([self.LATE])[0])
        self.assertTrue(line.startswith("RUNNER: the sleep returned late"), line)
        self.assertIn("behind=8552 us oversleep=8401 us", line)

    def test_a_late_wake_with_a_short_oversleep_is_ours(self):
        f = sv.pump_seconds([self.LATE.replace("oversleep_max_us=8401", "oversleep_max_us=900")])[0]
        self.assertTrue(sv.split_verdict(f).startswith("OURS: the wake was late"), sv.split_verdict(f))

    def test_an_overrun_is_judged_against_the_quiet_host_block(self):
        base = self.LATE.replace("cause=late", "cause=none").replace("rebootstraps=1", "rebootstraps=0")
        base = base.replace("overruns=0", "overruns=1")
        # run 43990478131: process 3212, cpu 3214 (this VM counts steal as CPU): still the runner
        steal = sv.pump_seconds(
            [base.replace("max_process_us=57", "max_process_us=3212").replace("max_process_cpu_us=49", "max_process_cpu_us=3214")]
        )[0]
        # no witnessed host stall that second: unproven, never a silent RUNNER
        self.assertTrue(sv.split_verdict(steal).startswith("UNPROVEN: a 3212 us block"), sv.split_verdict(steal))
        # the edge: 10 x 56 = 560 us is still ours, one more us is past it
        edge = sv.pump_seconds([base.replace("max_process_us=57", "max_process_us=560")])[0]
        self.assertTrue(sv.split_verdict(edge).startswith("OURS: a block's own work"), sv.split_verdict(edge))
        past = sv.pump_seconds([base.replace("max_process_us=57", "max_process_us=561")])[0]
        self.assertTrue(sv.split_verdict(past).startswith("UNPROVEN"), sv.split_verdict(past))

    def test_a_long_block_in_a_witnessed_host_stall_is_runner(self):
        line = (
            "[200] 2026-11-17T01:10:00.900Z dante-input dlink:  WARN dstack::stream::watch: engine "
            "pump lost or overran blocks this second device=\"obs-ci\" rebootstraps=0 missed=0 "
            "overruns=1 writes_after_send=0 max_process_us=3212 cause=none behind_max_us=2145 "
            "oversleep_max_us=291 drain_max_us=63 drain_max_datagrams=10 max_process_cpu_us=3214"
        )
        (f,) = sv.pump_seconds([line])
        day = sv.utc_ms("2026-11-17T00:00:00.000")
        host = sv.stalls(["01:10:00.412345 stall 6.2 ms", "01:12:00.000000 stall 9.0 ms"], day)
        self.assertEqual(host[0], (sv.utc_ms("2026-11-17T01:10:00.412"), 6.2))
        v = sv.split_verdict(f, host)
        self.assertIn("RUNNER: a 3212 us block in a second the witness saw the host stall 6.2 ms", v)
        # a stall outside the second before the line does not count
        v2 = sv.split_verdict(f, host[1:])
        self.assertIn("UNPROVEN", v2)

    def test_a_clock_re_bootstrap_is_ours_with_its_cause(self):
        f = sv.pump_seconds([self.LATE.replace("cause=late", "cause=clock_back+retime")])[0]
        line = sv.split_verdict(f)
        self.assertTrue(
            line.startswith("OURS: a re-bootstrap on clock_back+retime"), line
        )
        both = sv.pump_seconds([self.LATE.replace("cause=late", "cause=late+reanchor")])[0]
        line = sv.split_verdict(both)
        self.assertIn("RUNNER: the sleep returned late", line)
        self.assertIn("OURS: a re-bootstrap on reanchor", line)

    def test_a_warned_second_no_rule_reads_is_unclassified(self):
        f = sv.pump_seconds([self.LATE.replace("cause=late", "cause=none")])[0]
        self.assertTrue(sv.split_verdict(f).startswith("UNCLASSIFIED: cause=none"), sv.split_verdict(f))

    def test_an_unmeasured_block_is_said(self):
        f = sv.pump_seconds(
            [self.LATE.replace("overruns=0", "overruns=1").replace("max_process_us=57", "max_process_us=-")]
        )[0]
        self.assertIn("UNMEASURED overrun", sv.split_verdict(f))

    def test_a_timed_obs_line_gives_its_time(self):
        line = "[200] 2026-11-17T01:08:49.612Z " + self.LATE.split("] ", 1)[1]
        (f,) = sv.pump_seconds([line])
        self.assertEqual(f["_ms"], sv.utc_ms("2026-11-17T01:08:49.612"))
        self.assertTrue(sv.split_verdict(f).startswith("01:08:49.612 RUNNER"), sv.split_verdict(f))

    def test_a_break_after_a_tx_schedule_restart_is_runner(self):
        obs = [
            "[200] 2026-11-17T01:08:49.603Z dante-input dlink:  WARN dstack::media::tx: "
            "the TX schedule starts again lag=827 max_lag=803"
        ]
        restarts = sv.tx_restarts(obs)
        self.assertEqual(restarts, [(sv.utc_ms("2026-11-17T01:08:49.603"), "lag=827 max_lag=803")])
        # a 9.02 ms silence reported at .622: it began at .613, 10 ms after the restart
        (line,) = sv.verdict([(sv.utc_ms("2026-11-17T01:08:49.622"), 9.0208)], [], restarts)
        self.assertTrue(line.startswith("RUNNER: our TX thread ran late at 01:08:49.622"), line)
        self.assertIn("starting +10 ms after the TX schedule started again, lag=827 max_lag=803", line)

    def test_a_break_before_the_peer_s_own_pump_skip_is_peer(self):
        peer = [
            '2026-11-17T01:08:40.225376Z  WARN dstack::stream::watch: engine pump lost or overran '
            'blocks this second device="ci-peer" rebootstraps=1 missed=5 overruns=0 cause=late',
            '2026-11-17T01:09:00.000000Z  WARN dstack::stream::watch: engine pump lost or overran '
            'blocks this second device="ci-peer" rebootstraps=0 missed=0 overruns=1 cause=none',
        ]
        skips = sv.peer_skips(peer)
        self.assertEqual(skips, [sv.utc_ms("2026-11-17T01:08:40.225")])
        (line,) = sv.verdict([(sv.utc_ms("2026-11-17T01:08:39.070"), 0.0208, "misplaced")], [], [], skips)
        self.assertTrue(line.startswith("PEER: a 0.02 ms tone break at 01:08:39.070, 1155 ms before"), line)
        # past the watch's 2 s, or after the skip's warning, it is not the peer's
        (late,) = sv.verdict([(sv.utc_ms("2026-11-17T01:08:38.000"), 0.0208, "misplaced")], [], [], skips)
        self.assertTrue(late.startswith("OURS:"), late)
        (after,) = sv.verdict([(sv.utc_ms("2026-11-17T01:08:41.000"), 0.0208, "misplaced")], [], [], skips)
        self.assertTrue(after.startswith("OURS:"), after)
        # a watch line just before the break's report (its second boundary inside the break)
        (edge,) = sv.verdict([(sv.utc_ms("2026-11-17T01:08:40.300"), 0.0208, "misplaced")], [], [], skips)
        self.assertTrue(edge.startswith("PEER:"), edge)
        (past,) = sv.verdict([(sv.utc_ms("2026-11-17T01:08:40.326"), 0.0208, "misplaced")], [], [], skips)
        self.assertTrue(past.startswith("OURS:"), past)
        # our own TX restart before a misplaced break wins over a peer skip after it
        (ours,) = sv.verdict(
            [(sv.utc_ms("2026-11-17T01:08:39.070"), 0.0208, "misplaced")],
            [],
            [(sv.utc_ms("2026-11-17T01:08:39.060"), "lag=900 max_lag=803")],
            skips,
        )
        self.assertTrue(ours.startswith("RUNNER: our TX thread ran late"), ours)
        # a silence near a peer skip is not the peer's (its skip reads as misplaced)
        (silence,) = sv.verdict([(sv.utc_ms("2026-11-17T01:08:39.872"), 10.35, "silence")], [], [], skips)
        self.assertTrue(silence.startswith("OURS:"), silence)

    def test_breaks_carry_their_kind(self):
        peer = [
            "2026-11-17T01:08:39.070645Z  WARN dstack::peer::tone: dstack-dev peer: a tone break "
            "rx_channel=1 samples=2 ms=0.020833333333333332 gaps=3 kind=misplaced",
            "2026-11-17T01:08:39.070705Z  WARN dstack::peer::tone: dstack-dev peer: a tone break "
            "rx_channel=2 samples=2 ms=0.020833333333333332 gaps=3 kind=misplaced",
        ]
        self.assertEqual(
            sv.breaks(peer, 0, 2**62, with_kind=True),
            [(sv.utc_ms("2026-11-17T01:08:39.070"), 0.020833333333333332, "misplaced")],
        )

    def test_a_pump_wake_wins_over_a_tx_restart(self):
        w = [(10_000, 9_000, "re-bootstrapped: blocks lost")]
        (line,) = sv.verdict([(10_030, 5.0)], w, [(10_010, "lag=900 max_lag=803")])
        self.assertTrue(line.startswith("RUNNER: engine pump descheduled"), line)

    def test_minutes(self):
        self.assertEqual(sv.minutes(OBS), [8604])

    def test_the_cli_prints_the_verdict_and_exits_zero(self):
        with tempfile.TemporaryDirectory() as d:
            peer, obs = os.path.join(d, "peer.log"), os.path.join(d, "soak.log")
            with open(peer, "w") as f:
                f.write("\n".join(PEER) + "\n")
            with open(obs, "w") as f:
                f.write("\n".join(OBS + [self.LATE]) + "\n")
            r = subprocess.run(
                [sys.executable, "-I", sv.__file__, "--peer-log", peer, "--obs-log", obs,
                 "--from-ms", str(FROM), "--to-ms", str(TO)],
                capture_output=True, text=True, check=True,
            )
        self.assertIn("== the verdict on the soak's tone breaks (2)", r.stdout)
        self.assertIn("RUNNER: engine pump descheduled 8.6 ms", r.stdout)
        self.assertIn("OURS: a 7.75 ms tone break", r.stdout)
        self.assertIn("latest wake per minute, us: 8604", r.stdout)
        self.assertIn("== the pump's lost seconds (1, dlink's pump watch, dstack#354)", r.stdout)
        self.assertIn("RUNNER: the sleep returned late", r.stdout)


if __name__ == "__main__":
    unittest.main()
