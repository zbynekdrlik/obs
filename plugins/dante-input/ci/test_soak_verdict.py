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
            "OURS: a 7.75 ms tone break reported at 21:11:00.000 with no pump late wake "
            "-50..+250 ms before its start",
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

    def test_an_overrun_off_the_cpu_is_runner_one_at_work_is_ours(self):
        base = self.LATE.replace("cause=late", "cause=none").replace("rebootstraps=1", "rebootstraps=0")
        base = base.replace("overruns=0", "overruns=1")
        off = sv.pump_seconds([base.replace("max_process_us=57", "max_process_us=2080")])[0]
        self.assertTrue(sv.split_verdict(off).startswith("RUNNER: a block off the CPU"), sv.split_verdict(off))
        work = sv.pump_seconds(
            [base.replace("max_process_us=57", "max_process_us=2080").replace("max_process_cpu_us=49", "max_process_cpu_us=1900")]
        )[0]
        self.assertTrue(sv.split_verdict(work).startswith("OURS: a block's own work"), sv.split_verdict(work))
        # the edge: 4 x cpu + 500 is still work, one more us is off the CPU
        edge = base.replace("max_process_us=57", "max_process_us=696")
        self.assertTrue(sv.split_verdict(sv.pump_seconds([edge])[0]).startswith("OURS"))
        past = base.replace("max_process_us=57", "max_process_us=697")
        self.assertTrue(sv.split_verdict(sv.pump_seconds([past])[0]).startswith("RUNNER"))

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

    def test_an_unmeasured_cpu_is_said(self):
        f = sv.pump_seconds(
            [self.LATE.replace("overruns=0", "overruns=1").replace("max_process_cpu_us=49", "max_process_cpu_us=-")]
        )[0]
        self.assertIn("UNMEASURED overrun", sv.split_verdict(f))

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
