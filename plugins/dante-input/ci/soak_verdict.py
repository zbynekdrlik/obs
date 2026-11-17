#!/usr/bin/env python3
"""The soak's verdict on each peer tone break: RUNNER or OURS (obs#71, dstack#354).

A tone break the peer heard within a match window of one of dante-input's own pump late wakes
(`dante-input pump: a late wake at <UTC> behind <us> us ...`) is the runner holding the pump:
`RUNNER: engine pump descheduled X ms at T`. A break with no such wake is `OURS`. Either way the
run still fails (it tested nothing there); the verdict only says whether a rerun is the answer.

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
    r"samples=\d+ ms=([0-9.]+)"
)
WAKE = re.compile(
    r"dante-input pump: a late wake at (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3})Z "
    r"behind (\d+) us \(([^)]*)\)"
)
MINUTE = re.compile(r"dante-input pump: the last minute's latest wake (\d+) us behind")
# dlink's pump watch (dstack v0.6.159+, dstack#354): one warning a second that lost blocks or
# overran, with where the time went.
PUMP_WARNING = "engine pump lost or overran blocks this second"
FIELD = re.compile(r"\b([a-z_]+)=(\"[^\"]*\"|\S+)")
# A block whose wall time is past this many times its thread CPU (plus a floor) was off the CPU.
PREEMPT_RATIO = 4
PREEMPT_FLOOR_US = 500


def pump_seconds(obs_lines):
    """dlink's pump-watch warnings as field dicts (values as text)."""
    out = []
    for line in obs_lines:
        if PUMP_WARNING in line:
            out.append({k: v.strip('"') for k, v in FIELD.findall(line.split(PUMP_WARNING, 1)[1])})
    return out


def num(fields, key):
    v = fields.get(key)
    return int(v) if v is not None and v.isdigit() else None


def split_verdict(fields):
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
        if proc is None or cpu is None:
            words.append("UNMEASURED overrun (no thread CPU time)")
        elif proc > PREEMPT_RATIO * cpu + PREEMPT_FLOOR_US:
            words.append("RUNNER: a block off the CPU (wall far above its thread CPU)")
        else:
            words.append("OURS: a block's own work took the time")
    if not words:
        words.append(f"UNCLASSIFIED: cause={cause}, nothing the rules above read")
    return "; ".join(words) + f" [{facts}]"


def utc_ms(stamp):
    """`YYYY-MM-DDTHH:MM:SS.ffffff` (UTC) to ms since the epoch."""
    head, frac = stamp.split(".")
    t = datetime.datetime.strptime(head, "%Y-%m-%dT%H:%M:%S").replace(
        tzinfo=datetime.timezone.utc
    )
    return int(t.timestamp()) * 1000 + int((frac + "000")[:3])


def breaks(peer_lines, from_ms, to_ms):
    """The peer's tone breaks in [from_ms, to_ms], one per time (both channels break together)."""
    seen = {}
    for line in peer_lines:
        m = BREAK.search(line)
        if not m:
            continue
        at = utc_ms(m.group(1))
        if from_ms <= at <= to_ms:
            # channel 2's line of the same break comes microseconds after channel 1's
            if not any(abs(at - t) <= 1 for t in seen):
                seen[at] = float(m.group(3))
    return sorted(seen.items())


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


def verdict(brks, wks):
    """One line per break: RUNNER with the matching wake (the nearest to the break's start, the
    report time less its length), else OURS."""
    out = []
    for at, ms in brks:
        start = at - round(ms)
        near = [w for w in wks if -MATCH_BEFORE_MS <= start - w[0] <= MATCH_AFTER_MS]
        when = datetime.datetime.fromtimestamp(at / 1000, datetime.timezone.utc)
        t = when.strftime("%H:%M:%S.") + f"{at % 1000:03d}"
        if near:
            w = min(near, key=lambda w: abs(start - w[0]))
            out.append(
                f"RUNNER: engine pump descheduled {w[1] / 1000:.1f} ms at {t} "
                f"(a {ms:.2f} ms tone break starting {start - w[0]:+d} ms after the pump's "
                f"late wake, {w[2]})"
            )
        else:
            out.append(
                f"OURS: a {ms:.2f} ms tone break reported at {t} with no pump late wake "
                f"-{MATCH_BEFORE_MS}..+{MATCH_AFTER_MS} ms before its start"
            )
    return out


def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--peer-log", required=True)
    p.add_argument("--obs-log", required=True)
    p.add_argument("--from-ms", type=int, required=True)
    p.add_argument("--to-ms", type=int, required=True)
    a = p.parse_args(argv)
    with open(a.peer_log, encoding="utf-8", errors="replace") as f:
        peer = f.read().splitlines()
    with open(a.obs_log, encoding="utf-8", errors="replace") as f:
        obs = f.read().splitlines()
    brks = breaks(peer, a.from_ms, a.to_ms)
    wks = wakes(obs)
    print(f"== the pump's late wakes ({len(wks)}, dante-input, obs#71)")
    for at, us, note in wks:
        when = datetime.datetime.fromtimestamp(at / 1000, datetime.timezone.utc)
        print(f"{when.strftime('%H:%M:%S.')}{at % 1000:03d} late wake {us / 1000:.2f} ms ({note})")
    print("== the pump's latest wake per minute, us: " + (" ".join(map(str, minutes(obs))) or "(none)"))
    secs = pump_seconds(obs)
    print(f"== the pump's lost seconds ({len(secs)}, dlink's pump watch, dstack#354)")
    for fields in secs:
        print(split_verdict(fields))
    print(f"== the verdict on the soak's tone breaks ({len(brks)})")
    for line in verdict(brks, wks):
        print(line)
    if not brks:
        print("(no tone break in the soak's run)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
