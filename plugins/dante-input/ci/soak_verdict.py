#!/usr/bin/env python3
"""The soak's verdict on each peer tone break: RUNNER, PEER or OURS (obs#71, dstack#354).

Each break the peer heard is set beside, in this order:
- dante-input's own pump late wakes (`dante-input pump: a late wake at <UTC> ...`): RUNNER, the
  runner held our pump;
- dlink's `the TX schedule starts again` (our TX thread ran past the receivers' latency and late
  packets were dropped by design, dstack#354): RUNNER, the runner held our TX thread;
- the test peer's own pump re-bootstraps (its pump watch in peer.log; a skip there reads as a
  2-sample `kind=misplaced` break, dstack#358): PEER, the test peer's own fault;
- else OURS.
The run still fails on any of them (it tested nothing there); the verdict only says where to look.
The OBS log's lines carry the module test's UTC time (`[level] <UTC>Z ...`, obs#71).

Usage: soak_verdict.py --peer-log peer.log --obs-log soak.log --from-ms A --to-ms B
Only breaks between A and B (UTC ms since the epoch, the main soak's run) are judged.
"""

import argparse
import datetime
import re
import sys

# The peer reports a break once the tone is back, so the break STARTED its length before the report
# (a 1 s gap reports 1 s after its start). The start is matched: it comes after the pump's late
# wake by at least a Dante latency (10 ms in the soak) plus the peer's own block. The offset is
# printed, so the window can be tightened once real pairs are read.
MATCH_BEFORE_MS = 50
MATCH_AFTER_MS = 250

BREAK = re.compile(
    r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z .*a tone break rx_channel=(\d+) "
    r"samples=\d+ ms=([0-9.]+)(?:.* kind=(\w+))?"
)
WAKE = re.compile(
    r"dante-input pump: a late wake at (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3})Z "
    r"behind (\d+) us \(([^)]*)\)"
)
MINUTE = re.compile(r"dante-input pump: the last minute's latest wake (\d+) us behind")
# the module test's log line: `[level] <UTC>Z <text>`
OBS_TIME = re.compile(r"^\[\d+\] (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3})Z ")
TX_RESTART = "the TX schedule starts again"
PEER_TIME = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z ")
# The peer's pump watch logs once a second for the second before: a skip it reports came up to
# this long before its warning.
PEER_WATCH_MS = 2000
# ... and its second boundary can fall inside a short break, so its line may come just before the
# break's report.
PEER_BEFORE_MS = 100
# the stall witness's line: `HH:MM:SS.ffffff stall X ms` (UTC, the run's date)
STALL = re.compile(r"^(\d{2}):(\d{2}):(\d{2})\.(\d+) stall ([0-9.]+) ms")
# obs's block on a quiet host (dstack#354, measured on obs's shape: p50 26, max 56 us, also right
# after a re-bootstrap, a Rated restart and a TX resync). The runner (an Azure VM without paravirt
# steal accounting) counts host steal as the thread's CPU, so cpu = wall proves nothing there.
QUIET_BLOCK_MAX_US = 56
# dlink's pump watch (dstack v0.6.159+, dstack#354): one warning a second that lost blocks or
# overran, with where the time went.
PUMP_WARNING = "engine pump lost or overran blocks this second"
FIELD = re.compile(r"\b([a-z_]+)=(\"[^\"]*\"|\S+)")
# A block over this many times the quiet-host block is not obs's or dlink's work.
STEAL_RATIO = 10


def line_ms(line, pattern=OBS_TIME):
    m = pattern.search(line)
    return utc_ms(m.group(1)) if m else None


def pump_seconds(obs_lines):
    """dlink's pump-watch warnings as field dicts (values as text); `_ms` is the line's time."""
    out = []
    for line in obs_lines:
        if PUMP_WARNING in line:
            f = {k: v.strip('"') for k, v in FIELD.findall(line.split(PUMP_WARNING, 1)[1])}
            f["_ms"] = line_ms(line)
            out.append(f)
    return out


def tx_restarts(obs_lines):
    """The times dlink's TX schedule started again, with its lag text."""
    out = []
    for line in obs_lines:
        if TX_RESTART in line:
            at = line_ms(line)
            if at is not None:
                out.append((at, line.split(TX_RESTART, 1)[1].strip()))
    return out


def peer_skips(peer_lines):
    """The test peer's own pump-watch warnings that lost blocks: their times."""
    out = []
    for line in peer_lines:
        if PUMP_WARNING in line:
            f = dict(FIELD.findall(line.split(PUMP_WARNING, 1)[1]))
            at = line_ms(line, PEER_TIME)
            if at is not None and (f.get("missed", "0") != "0" or f.get("rebootstraps", "0") != "0"):
                out.append(at)
    return out


def num(fields, key):
    v = fields.get(key)
    return int(v) if v is not None and v.isdigit() else None


def stalls(stall_lines, day_ms):
    """The witness's host stalls as (UTC ms, ms); `day_ms` is the run's UTC midnight."""
    out = []
    for line in stall_lines:
        m = STALL.search(line)
        if m:
            h, mi, se = int(m.group(1)), int(m.group(2)), int(m.group(3))
            ms = int((m.group(4) + "000")[:3])
            out.append((day_ms + ((h * 60 + mi) * 60 + se) * 1000 + ms, float(m.group(5))))
    return out


def split_verdict(fields, host_stalls=()):
    """One line for a pump-watch second: where its lost time went, RUNNER or OURS."""
    behind = num(fields, "behind_max_us")
    over = num(fields, "oversleep_max_us")
    proc = num(fields, "max_process_us")
    cpu = num(fields, "max_process_cpu_us")
    drain, dgrams = num(fields, "drain_max_us"), num(fields, "drain_max_datagrams")
    cause = fields.get("cause", "?")
    facts = (
        f"cause={cause} behind={behind} us oversleep={over} us process={proc} us "
        f"cpu={fields.get('max_process_cpu_us', '?')} us drain={drain} us ({dgrams} datagrams)"
    )
    words = []
    if "late" in cause.split("+"):
        if behind is None or over is None:
            words.append("UNMEASURED late wake")
        elif over * 2 >= behind:
            words.append("RUNNER: the sleep returned late (the scheduler ran the pump late)")
        else:
            words.append("OURS: the wake was late but the sleep was not (time went to work before it)")
    # a re-bootstrap on a clock or latency event is the stack's own reaction, not the scheduler
    other = [c for c in cause.split("+") if c not in ("late", "none", "?")]
    if other:
        words.append(
            f"OURS: a re-bootstrap on {'+'.join(other)} (a clock or latency event, not the scheduler)"
        )
    blocks = (num(fields, "overruns") or 0) + (num(fields, "writes_after_send") or 0)
    if blocks:
        if proc is None:
            words.append("UNMEASURED overrun (no block time)")
        elif proc > STEAL_RATIO * QUIET_BLOCK_MAX_US:
            at = fields.get("_ms")
            seen = [] if at is None else [x for x in host_stalls if at - 1000 <= x[0] <= at]
            if seen:
                worst = max(x[1] for x in seen)
                words.append(
                    f"RUNNER: a {proc} us block in a second the witness saw the host stall "
                    f"{worst:.1f} ms (obs's block is at most {QUIET_BLOCK_MAX_US} us on a quiet host)"
                )
            else:
                words.append(
                    f"UNPROVEN: a {proc} us block with no host stall witnessed that second: steal "
                    f"(this VM counts it as the thread's CPU) or ours (obs's block is at most "
                    f"{QUIET_BLOCK_MAX_US} us on a quiet host)"
                )
        else:
            words.append("OURS: a block's own work took the time")
    if not words:
        words.append(f"UNCLASSIFIED: cause={cause}, nothing the rules above read")
    at = fields.get("_ms")
    when = "" if at is None else hhmmss(at) + " "
    return when + "; ".join(words) + f" [{facts}]"


def hhmmss(at):
    when = datetime.datetime.fromtimestamp(at / 1000, datetime.timezone.utc)
    return when.strftime("%H:%M:%S.") + f"{at % 1000:03d}"


def utc_ms(stamp):
    """`YYYY-MM-DDTHH:MM:SS.ffffff` (UTC) to ms since the epoch."""
    head, frac = stamp.split(".")
    t = datetime.datetime.strptime(head, "%Y-%m-%dT%H:%M:%S").replace(
        tzinfo=datetime.timezone.utc
    )
    return int(t.timestamp()) * 1000 + int((frac + "000")[:3])


def breaks(peer_lines, from_ms, to_ms, with_kind=False):
    """The peer's tone breaks in [from_ms, to_ms], one per time (both channels break together):
    (UTC ms, its ms[, its kind: silence / misplaced / None before dstack-dev v0.6.144])."""
    seen = {}
    for line in peer_lines:
        m = BREAK.search(line)
        if not m:
            continue
        at = utc_ms(m.group(1))
        if from_ms <= at <= to_ms:
            # channel 2's line of the same break comes microseconds after channel 1's
            if not any(abs(at - t) <= 1 for t in seen):
                seen[at] = (float(m.group(3)), m.group(4))
    out = sorted(seen.items())
    if with_kind:
        return [(at, ms, kind) for at, (ms, kind) in out]
    return [(at, ms) for at, (ms, _) in out]


def wakes(obs_lines):
    """dante-input's late wakes: (UTC ms, µs behind, its note)."""
    out = []
    for line in obs_lines:
        m = WAKE.search(line)
        if m:
            out.append((utc_ms(m.group(1)), int(m.group(2)), m.group(3)))
    return out


def minutes(obs_lines):
    return [int(m.group(1)) for m in map(MINUTE.search, obs_lines) if m]


def verdict(brks, wks, restarts=(), skips=()):
    """One line per break (its start = the report time less its length): RUNNER on a pump late
    wake or a TX schedule restart just before the start, PEER on the test peer's own pump skip,
    else OURS."""
    out = []
    for b in brks:
        at, ms = b[0], b[1]
        kind = b[2] if len(b) > 2 else None
        start = at - round(ms)
        t = hhmmss(at)
        near = [w for w in wks if -MATCH_BEFORE_MS <= start - w[0] <= MATCH_AFTER_MS]
        tx = [r for r in restarts if -MATCH_BEFORE_MS <= start - r[0] <= MATCH_AFTER_MS]
        # a peer skip reads as one 2-sample misplaced break (dstack#358), never a silence
        peer = (
            [p for p in skips if -PEER_BEFORE_MS <= p - at <= PEER_WATCH_MS]
            if kind == "misplaced"
            else []
        )
        if near:
            w = min(near, key=lambda w: abs(start - w[0]))
            out.append(
                f"RUNNER: engine pump descheduled {w[1] / 1000:.1f} ms at {t} "
                f"(a {ms:.2f} ms tone break starting {start - w[0]:+d} ms after the pump's "
                f"late wake, {w[2]})"
            )
        elif tx:
            r = min(tx, key=lambda r: abs(start - r[0]))
            out.append(
                f"RUNNER: our TX thread ran late at {t} (a {ms:.2f} ms tone break starting "
                f"{start - r[0]:+d} ms after the TX schedule started again, {r[1]}: late packets "
                "dropped by design)"
            )
        elif peer:
            out.append(
                f"PEER: a {ms:.2f} ms tone break at {t}, {min(peer) - at} ms before the test "
                "peer's own pump lost blocks (dstack#358, not obs)"
            )
        else:
            out.append(
                f"OURS: a {ms:.2f} ms tone break reported at {t} with no pump late wake, no TX "
                f"schedule restart -{MATCH_BEFORE_MS}..+{MATCH_AFTER_MS} ms before its start and "
                "no peer pump skip after it"
            )
    return out


def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--peer-log", required=True)
    p.add_argument("--obs-log", required=True)
    p.add_argument("--from-ms", type=int, required=True)
    p.add_argument("--to-ms", type=int, required=True)
    p.add_argument("--stall-log", help="the stall witness's log (HH:MM:SS UTC on the run's day)")
    a = p.parse_args(argv)
    with open(a.peer_log, encoding="utf-8", errors="replace") as f:
        peer = f.read().splitlines()
    with open(a.obs_log, encoding="utf-8", errors="replace") as f:
        obs = f.read().splitlines()
    brks = breaks(peer, a.from_ms, a.to_ms, with_kind=True)
    wks = wakes(obs)
    print(f"== the pump's late wakes ({len(wks)}, dante-input, obs#71)")
    for at, us, note in wks:
        when = datetime.datetime.fromtimestamp(at / 1000, datetime.timezone.utc)
        print(f"{when.strftime('%H:%M:%S.')}{at % 1000:03d} late wake {us / 1000:.2f} ms ({note})")
    print("== the pump's latest wake per minute, us: " + (" ".join(map(str, minutes(obs))) or "(none)"))
    host = []
    if a.stall_log:
        day = a.from_ms - a.from_ms % 86_400_000
        with open(a.stall_log, encoding="utf-8", errors="replace") as f:
            host = stalls(f.read().splitlines(), day)
    secs = pump_seconds(obs)
    print(f"== the pump's lost seconds ({len(secs)}, dlink's pump watch, dstack#354)")
    for fields in secs:
        print(split_verdict(fields, host))
    print(f"== the verdict on the soak's tone breaks ({len(brks)})")
    for line in verdict(brks, wks, tx_restarts(obs), peer_skips(peer)):
        print(line)
    if not brks:
        print("(no tone break in the soak's run)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
