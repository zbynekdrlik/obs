//! The `dante-input status:` line, every 1 s for the first 30 s, then every 10 s (obs#1 log contract). Counters are totals since the
//! start. Audio reached OBS whole while rx_loss, rx_holes and underruns stay put: rx_loss counts
//! frames the network never brought, rx_holes samples the pump read before their packet came (OBS
//! got silence there), underruns blocks the pump skipped (OBS never got them). late_blocks and
//! overruns are timing only: a block run up to the margin late still reads its own samples.
//! wire_late, wake_delay_us and wake_delay_peak_us (dlink v0.6.101, dstack #234) tell a late
//! packet late on the wire from our RX thread's late wake; they were appended after max_obs_us, so
//! the tokens before them keep their order (camera-box#1510). New tokens always go last. The wake delay is the RX pass's time less
//! the kernel's receive stamp, which is wall clock (CLOCK_REALTIME): a forward step of it between a
//! receive and the pass reads as a false wake delay, and stays in wake_delay_peak_us for the run.
//! Since dlink v0.6.125 (dstack #278) a reordered packet fills the hole its overtaker opened and
//! counts in rx_reordered, appended after tx_priming (obs#45); rx_gaps / rx_loss count only the
//! holes still open at playout. Since dlink v0.6.126 a ring hole read before its flow's first
//! sample (the OBS-start burst) counts in start_holes, appended after rx_reordered (obs#47), not
//! in rx_holes. The wake and write delays' minute percentiles are not on the line: dlink logs them
//! once a minute per flow (`RX flow timing in the last minute ...`, dstack #234), through our
//! logger.

use std::fmt::Write as _;

/// One reading of the device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The pump runs on the media clock (not free pacing).
    pub media_clock: bool,
    /// The clock poller's last poll was in holdover.
    pub holdover: bool,
    pub clock_polls: u64,
    pub re_anchors: u32,
    pub grandmaster: Option<[u8; 6]>,
    pub rx_packets: u64,
    /// Frames that never came (the gaps' frames).
    pub rx_loss: u64,
    pub rx_gaps: u64,
    /// Packets later than the flow's latency.
    pub rx_late: u64,
    /// Datagrams the kernel dropped at the flow sockets.
    pub socket_drops: u64,
    /// Samples the pump read that no packet had written: silence reached OBS there.
    pub rx_holes: u64,
    /// Samples whose packet came after the pump had read past them (each also a hole).
    pub rx_written_late: u64,
    pub blocks: u64,
    /// Blocks the pump skipped (a re-bootstrap): never delivered.
    pub underruns: u64,
    /// Blocks run one block or more after their due time, within the margin: delivered whole.
    pub late_blocks: u64,
    pub rebootstraps: u64,
    /// Blocks whose own time passed one block period.
    pub overruns: u64,
    /// The longest block, µs.
    pub max_process_us: u64,
    /// Frames lost because the ring to the OBS feeder was full.
    pub obs_dropped: u64,
    /// The longest callback into OBS, µs (the feeder thread, not the pump).
    pub max_obs_us: u64,
    /// Frames handed to OBS.
    pub delivered_frames: u64,
    /// Packets already later than the flow's limit when the kernel received them (all flows):
    /// late on the wire, not by a late RX-thread wake.
    pub wire_late: u64,
    /// The largest RX-thread wake delay in dlink's last heartbeat second (any flow), µs: a
    /// one-second sample, not the whole print interval (1 s, then 10 s), so wake_delay_peak_us
    /// holds a spike between prints. dlink takes the second only with its clock ready; before that
    /// it keeps the last one.
    pub wake_delay_us: u64,
    /// The largest RX-thread wake delay since the start (any flow), µs.
    pub wake_delay_peak_us: u64,
    /// The OBS track the TX carries, from 1; 0 without TX (obs#41). The tx_* tokens are 0 then.
    pub tx_track: u32,
    /// Frames the TX player gave the device.
    pub tx_played: u64,
    /// TX blocks with no OBS audio in the FIFO: silence went out.
    pub tx_underruns: u64,
    /// FIFO depths the TX player could not steer back (a skip or a refill).
    pub tx_recentres: u64,
    /// OBS frames the TX FIFO had no room for.
    pub tx_overflows: u64,
    /// The TX converter's correction, tenths of a ppm (printed as ppm with one decimal).
    pub tx_ppm_tenths: i64,
    /// The TX player waits for the FIFO's depth: silence goes out.
    pub tx_priming: bool,
    /// Packets that filled a hole another packet had opened: overtaken on the way, not lost
    /// (dlink v0.6.125, dstack #278). rx_gaps / rx_loss count only the holes still open at
    /// playout, so a reorder no longer reads as loss (camera-box issue 1508).
    pub rx_reordered: u64,
    /// Samples the pump read before their flow's first sample (the OBS-start burst, a new flow's
    /// start): never audio OBS was owed, so apart from rx_holes (dlink v0.6.126, dstack #234).
    pub start_holes: u64,
    /// TX blocks that did not start where the last ended: dlink's player re-primed (v0.6.136,
    /// dstack#303, obs#56).
    pub tx_resyncs: u64,
    /// Blocks whose play time moved over 1 ms off their media time since the block before, and
    /// the largest such deviation since the start, µs (dlink v0.6.137, dstack#305; obs#57 gate).
    pub play_at_steps: u64,
    pub play_at_max_dev_us: u64,
    /// RX chunks handed to OBS more than 1 ms off their continuation, and skips the full ring could
    /// not fill (dante-input's own, obs#53; obs#57 gate).
    pub stamp_jumps: u64,
    pub gaps_unfilled: u64,
    /// OBS's TX pushes that came over 1.5 times the last push's audio length after it, since the
    /// start, and the largest gap between two pushes since the last status line, µs (obs#56: tells
    /// a late writer, OBS's audio thread, from a reader-side underrun).
    pub tx_late_pushes: u64,
    pub tx_push_gap_max_us: u64,
    /// obs#94 (camera-box#1527): the smallest lead of a stamped RX chunk when it was handed to
    /// OBS (the end of its audio, stamp + length, less the time now), since the last status line,
    /// µs, signed (`none` with no stamped chunk in the window); and the stamped chunks handed at
    /// or after the end of their audio since the start.
    pub min_lead_us: Option<i64>,
    pub late_to_mix: u64,
}

impl Status {
    pub fn line(&self) -> String {
        let mut s = String::from("dante-input status:");
        let gm = match self.grandmaster {
            Some(g) => g.iter().fold(String::new(), |mut acc, b| {
                let _ = write!(acc, "{b:02x}");
                acc
            }),
            None => "none".to_string(),
        };
        let _ = write!(
            s,
            " clock={} holdover={} gm={gm} polls={} re_anchors={} rx_packets={} rx_loss={} \
             rx_gaps={} rx_late={} socket_drops={} blocks={} underruns={} overruns={} delivered_frames={} \
             rx_holes={} rx_written_late={} late_blocks={} rebootstraps={} max_process_us={} \
             obs_dropped={} max_obs_us={} wire_late={} wake_delay_us={} wake_delay_peak_us={} \
             tx_track={} tx_played={} tx_underruns={} tx_recentres={} tx_overflows={} tx_ppm={:.1} \
             tx_priming={} rx_reordered={} start_holes={} tx_resyncs={} play_at_steps={} \
             play_at_max_dev_us={} stamp_jumps={} gaps_unfilled={} tx_late_pushes={} \
             tx_push_gap_max_us={} min_lead_us={} late_to_mix={}",
            if self.media_clock { "media" } else { "free" },
            u8::from(self.holdover),
            self.clock_polls,
            self.re_anchors,
            self.rx_packets,
            self.rx_loss,
            self.rx_gaps,
            self.rx_late,
            self.socket_drops,
            self.blocks,
            self.underruns,
            self.overruns,
            self.delivered_frames,
            self.rx_holes,
            self.rx_written_late,
            self.late_blocks,
            self.rebootstraps,
            self.max_process_us,
            self.obs_dropped,
            self.max_obs_us,
            self.wire_late,
            self.wake_delay_us,
            self.wake_delay_peak_us,
            self.tx_track,
            self.tx_played,
            self.tx_underruns,
            self.tx_recentres,
            self.tx_overflows,
            self.tx_ppm_tenths as f64 / 10.0,
            u8::from(self.tx_priming),
            self.rx_reordered,
            self.start_holes,
            self.tx_resyncs,
            self.play_at_steps,
            self.play_at_max_dev_us,
            self.stamp_jumps,
            self.gaps_unfilled,
            self.tx_late_pushes,
            self.tx_push_gap_max_us,
            self.min_lead_us
                .map_or_else(|| "none".to_string(), |v| v.to_string()),
            self.late_to_mix,
        );
        s
    }

    /// Takes the TX's counts (obs#41).
    pub fn tx(&mut self, c: crate::tx::TxCounts) {
        self.tx_track = c.track;
        self.tx_played = c.played;
        self.tx_underruns = c.underruns;
        self.tx_recentres = c.recentres;
        self.tx_overflows = c.overflows;
        self.tx_ppm_tenths = (c.ppm * 10.0).round() as i64;
        self.tx_priming = c.priming;
        self.tx_resyncs = c.resyncs;
    }

    /// Reads the device's counts from `media`.
    pub fn read(media: &dlink::Media, delivered_frames: u64) -> Status {
        let clock = media.clock_stats.counts();
        let mut st = Status {
            holdover: clock.in_holdover,
            clock_polls: clock.polls,
            re_anchors: clock.re_anchors,
            grandmaster: media.clock_stats.grandmaster(),
            delivered_frames,
            ..Status::default()
        };
        for slot in 0..32 {
            let l = media.rx.lane(slot);
            st.lane(Lane {
                packets: l.packets,
                gap_frames: l.gap_frames,
                gaps: l.gaps,
                late: l.late,
                reordered: l.reordered,
            });
            st.flow(Flow {
                wire_late: l.wire_late,
                wake_delay_us: media.rx.last_wake_delay_us(slot),
                wake_delay_peak_us: l.wake_delay_peak_us,
            });
        }
        if let Some(drops) = media.rx.socket_drops() {
            st.socket_drops = drops.iter().sum();
        }
        for ring in media.rx.rings().iter() {
            let c = ring.counters();
            st.ring(Ring {
                holes: c.holes,
                written_late: c.late,
                start_holes: c.start_holes,
            });
        }
        if let Some(p) = media.engine() {
            st.pump(Pump {
                media: p.media,
                blocks: p.blocks,
                late: p.late,
                missed: p.missed,
                rebootstraps: p.rebootstraps,
                overruns: p.overruns,
                max_process_us: p.max_process_us,
                play_at_steps: p.play_at_steps,
                play_at_max_dev_us: p.play_at_max_dev_us,
            });
        }
        st
    }

    /// One flow slot's RX counts: they add up over the slots.
    pub fn lane(&mut self, l: Lane) {
        self.rx_packets += l.packets;
        self.rx_loss += l.gap_frames;
        self.rx_gaps += l.gaps;
        self.rx_late += l.late;
        self.rx_reordered += l.reordered;
    }

    /// One channel ring's counts: they add up over the rings.
    pub fn ring(&mut self, r: Ring) {
        self.rx_holes += r.holes;
        self.rx_written_late += r.written_late;
        self.start_holes += r.start_holes;
    }

    /// One flow slot's RX timing: wire_late adds up, the wake delays keep the worst flow's.
    pub fn flow(&mut self, f: Flow) {
        self.wire_late += f.wire_late;
        self.wake_delay_us = self.wake_delay_us.max(f.wake_delay_us);
        self.wake_delay_peak_us = self.wake_delay_peak_us.max(f.wake_delay_peak_us);
    }

    /// The engine pump's counts: only skipped blocks are underruns.
    pub fn pump(&mut self, p: Pump) {
        self.media_clock = p.media;
        self.blocks = p.blocks;
        self.underruns = p.missed;
        self.late_blocks = p.late;
        self.rebootstraps = p.rebootstraps;
        self.overruns = p.overruns;
        self.max_process_us = p.max_process_us;
        self.play_at_steps = p.play_at_steps;
        self.play_at_max_dev_us = p.play_at_max_dev_us;
    }
}

/// One flow slot's RX counts (dstack's RxCounts, which dlink does not name).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lane {
    pub packets: u64,
    /// Frames of the holes still open at playout (dstack #278).
    pub gap_frames: u64,
    pub gaps: u64,
    pub late: u64,
    /// Packets that filled a hole: reordered, not lost.
    pub reordered: u64,
}

/// One channel ring's counts (dstack's ring Counters, which dlink does not name).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ring {
    /// Samples read that no packet had written, once the flow ran (since dlink v0.6.126,
    /// dstack #234).
    pub holes: u64,
    pub written_late: u64,
    /// Samples read before the flow's first sample (dlink v0.6.126, dstack #234).
    pub start_holes: u64,
}

/// One flow slot's RX timing (dlink v0.6.101, dstack #234).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flow {
    pub wire_late: u64,
    /// The heartbeat's last second, µs.
    pub wake_delay_us: u64,
    /// Since the start, µs.
    pub wake_delay_peak_us: u64,
}

/// The engine pump's counts (dstack's PumpView, which dlink does not name).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pump {
    pub media: bool,
    pub blocks: u64,
    /// Blocks run one block or more late (within the margin).
    pub late: u64,
    /// Blocks skipped by re-bootstraps.
    pub missed: u64,
    pub rebootstraps: u64,
    pub overruns: u64,
    pub max_process_us: u64,
    /// dlink v0.6.137 (dstack#305).
    pub play_at_steps: u64,
    pub play_at_max_dev_us: u64,
}

/// One pump wake one block or more behind its due time (dstack's `stream::wake::LateWake`, which
/// dlink does not name; obs#71): its wall time, how far behind, whether the pump re-bootstrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LateWake {
    pub unix_ms: u64,
    pub behind_us: u64,
    pub rebootstrap: bool,
}

/// The most late-wake lines one status window logs; the rest are summed in one line.
pub const LATE_WAKE_LINES: usize = 16;

/// Logs each pump late wake once (obs#71). A wake is known by its time, lateness and outcome, not
/// by a time watermark, so a wall-clock step (the times are UTC) neither hides nor repeats one. The
/// pump keeps only its last 32; wakes its ring no longer held when read are counted from its
/// running total.
#[derive(Debug, Default)]
pub struct WakeLog {
    /// The wakes logged, the newest last; as many as the pump keeps, twice over.
    seen: std::collections::VecDeque<LateWake>,
    /// Wakes logged or counted lost since the start.
    accounted: u64,
    /// The pump the log follows (its stats' address): a new pump's count starts at 0.
    pump: usize,
}

impl WakeLog {
    const KEEP: usize = 64;

    /// The lines for this status window: `total` is the pump's running count of late wakes,
    /// read BEFORE `recent` (oldest first), so every wake it counts is in `recent` unless the
    /// ring dropped it. At most [`LATE_WAKE_LINES`] wake lines, then one counting the rest, and
    /// one for wakes lost from the ring. `pump` names the pump read: a new one (a device or engine
    /// restart, whose count starts at 0) starts the log again.
    pub fn lines(&mut self, pump: usize, total: u64, recent: &[LateWake]) -> Vec<String> {
        if pump != self.pump {
            *self = WakeLog {
                pump,
                ..WakeLog::default()
            };
        }
        let new: Vec<LateWake> = recent
            .iter()
            .filter(|w| !self.seen.contains(w))
            .copied()
            .collect();
        for w in &new {
            self.seen.push_back(*w);
            if self.seen.len() > Self::KEEP {
                self.seen.pop_front();
            }
        }
        self.accounted += new.len() as u64;
        let lost = total.saturating_sub(self.accounted);
        self.accounted += lost;
        let mut lines: Vec<String> = new
            .iter()
            .take(LATE_WAKE_LINES)
            .map(|w| {
                format!(
                    "dante-input pump: a late wake at {} behind {} us ({}) (obs#71)",
                    utc_ms(w.unix_ms),
                    w.behind_us,
                    if w.rebootstrap {
                        "re-bootstrapped: blocks lost"
                    } else {
                        "within the margin"
                    }
                )
            })
            .collect();
        if new.len() > LATE_WAKE_LINES {
            lines.push(format!(
                "dante-input pump: {} more late wake(s) in this window not listed (obs#71)",
                new.len() - LATE_WAKE_LINES
            ));
        }
        if lost > 0 {
            lines.push(format!(
                "dante-input pump: {lost} late wake(s) left the pump's last 32 before they were \
                 read; their times are lost (obs#71)"
            ));
        }
        lines
    }
}

/// obs#80: the line logged once when the TX thread first pulls OBS's track: the OBS frames pushed
/// before it (not written, no overflow: the device's media clock was not ready yet).
pub fn first_pull_line(unpulled: u64, rate: u32) -> String {
    let ms = if rate > 0 {
        unpulled as f64 * 1000.0 / f64::from(rate)
    } else {
        0.0
    };
    format!(
        "dante-input TX: the TX thread pulls OBS's track; {unpulled} OBS frames ({ms:.1} ms) before its first pull were not written (the media clock was not ready; obs#80)"
    )
}
/// The pump's latest wake over the last whole minute, logged once a minute (obs#71).
pub fn minute_wake_line(behind_us: u64) -> String {
    format!("dante-input pump: the last minute's latest wake {behind_us} us behind (obs#71)")
}

/// ms since the Unix epoch as UTC `YYYY-MM-DDTHH:MM:SS.mmmZ` (the civil-from-days algorithm).
pub fn utc_ms(unix_ms: u64) -> String {
    let secs = unix_ms / 1_000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600,
        rem / 60 % 60,
        rem % 60,
        unix_ms % 1_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_carries_every_contract_field_in_order() {
        let st = Status {
            media_clock: true,
            holdover: false,
            clock_polls: 12,
            re_anchors: 1,
            grandmaster: Some([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x0a]),
            rx_packets: 100,
            rx_loss: 3,
            rx_gaps: 1,
            rx_late: 2,
            socket_drops: 4,
            rx_holes: 7,
            rx_written_late: 8,
            blocks: 50,
            underruns: 5,
            late_blocks: 9,
            rebootstraps: 10,
            overruns: 6,
            max_process_us: 11,
            obs_dropped: 12,
            max_obs_us: 13,
            delivered_frames: 4800,
            wire_late: 14,
            wake_delay_us: 15,
            wake_delay_peak_us: 16,
            tx_track: 1,
            tx_played: 96000,
            tx_underruns: 17,
            tx_recentres: 18,
            tx_overflows: 19,
            tx_ppm_tenths: -125,
            tx_priming: false,
            rx_reordered: 20,
            start_holes: 21,
            tx_resyncs: 0,
            play_at_steps: 22,
            play_at_max_dev_us: 23,
            stamp_jumps: 24,
            gaps_unfilled: 25,
            tx_late_pushes: 26,
            tx_push_gap_max_us: 27,
            min_lead_us: Some(-28),
            late_to_mix: 29,
        };
        assert_eq!(
            st.line(),
            "dante-input status: clock=media holdover=0 gm=001dc1fffe0a polls=12 re_anchors=1 \
             rx_packets=100 rx_loss=3 rx_gaps=1 rx_late=2 socket_drops=4 blocks=50 underruns=5 \
             overruns=6 delivered_frames=4800 rx_holes=7 rx_written_late=8 late_blocks=9 \
             rebootstraps=10 max_process_us=11 obs_dropped=12 max_obs_us=13 \
             wire_late=14 wake_delay_us=15 wake_delay_peak_us=16 \
             tx_track=1 tx_played=96000 tx_underruns=17 tx_recentres=18 tx_overflows=19 \
             tx_ppm=-12.5 tx_priming=0 rx_reordered=20 start_holes=21 tx_resyncs=0 \
             play_at_steps=22 play_at_max_dev_us=23 stamp_jumps=24 gaps_unfilled=25 \
             tx_late_pushes=26 tx_push_gap_max_us=27 min_lead_us=-28 late_to_mix=29"
        );
    }

    /// camera-box#1510 (dstack #234): a late packet is late on the wire or woke our RX thread
    /// late. wire_late adds up over the flows; the wake delays are the worst flow's.
    #[test]
    fn the_flows_wire_late_add_up_and_their_wake_delays_take_the_worst() {
        let mut st = Status::default();
        st.flow(Flow {
            wire_late: 2,
            wake_delay_us: 300,
            wake_delay_peak_us: 4_000,
        });
        st.flow(Flow {
            wire_late: 5,
            wake_delay_us: 900,
            wake_delay_peak_us: 1_200,
        });
        st.flow(Flow::default());
        assert_eq!(
            (st.wire_late, st.wake_delay_us, st.wake_delay_peak_us),
            (7, 900, 4_000)
        );
        assert!(
            st.line().ends_with(
                " max_obs_us=0 wire_late=7 wake_delay_us=900 wake_delay_peak_us=4000 \
                 tx_track=0 tx_played=0 tx_underruns=0 tx_recentres=0 tx_overflows=0 tx_ppm=0.0 tx_priming=0 \
                 rx_reordered=0 start_holes=0 tx_resyncs=0 play_at_steps=0 play_at_max_dev_us=0 stamp_jumps=0 gaps_unfilled=0 tx_late_pushes=0 tx_push_gap_max_us=0 min_lead_us=none late_to_mix=0"
            ),
            "{}",
            st.line()
        );
    }

    /// strih-snv, v0.2.0 (camera-box#1485): ~200 late blocks/s with no skip read as ~200
    /// underruns/s. A late block is delivered whole; only a skipped one is lost.
    #[test]
    fn late_blocks_are_not_underruns_only_skipped_blocks_are() {
        let mut st = Status::default();
        st.pump(Pump {
            media: true,
            blocks: 508_915,
            late: 28_036,
            missed: 0,
            rebootstraps: 0,
            overruns: 1_292,
            max_process_us: 900,
            play_at_steps: 1,
            play_at_max_dev_us: 1_500,
        });
        assert_eq!((st.play_at_steps, st.play_at_max_dev_us), (1, 1_500));
        assert_eq!(st.underruns, 0);
        assert_eq!(st.late_blocks, 28_036);
        st.pump(Pump {
            missed: 3,
            rebootstraps: 1,
            ..Pump::default()
        });
        assert_eq!((st.underruns, st.rebootstraps, st.late_blocks), (3, 1, 0));
    }

    /// obs#80: the first-pull line names the frames and their length at OBS's rate.
    #[test]
    fn the_first_pull_line_names_the_frames_not_written() {
        assert_eq!(
            first_pull_line(48_000, 48_000),
            "dante-input TX: the TX thread pulls OBS's track; 48000 OBS frames (1000.0 ms) before its first pull were not written (the media clock was not ready; obs#80)"
        );
        assert!(first_pull_line(0, 0).contains(" 0 OBS frames (0.0 ms) "));
    }

    #[test]
    fn no_grandmaster_and_free_pacing_read_as_such() {
        let st = Status {
            holdover: true,
            ..Status::default()
        };
        let line = st.line();
        assert!(
            line.starts_with("dante-input status: clock=free holdover=1 gm=none "),
            "{line}"
        );
    }

    /// obs#45 (dstack #278): the slots' counts add up; a reorder is apart from the loss.
    #[test]
    fn the_slots_rx_counts_add_up_and_reorders_are_not_loss() {
        let mut st = Status::default();
        st.lane(Lane {
            packets: 100,
            gap_frames: 0,
            gaps: 0,
            late: 1,
            reordered: 7,
        });
        st.lane(Lane {
            packets: 50,
            gap_frames: 16,
            gaps: 1,
            late: 2,
            reordered: 3,
        });
        st.lane(Lane::default());
        assert_eq!(
            (
                st.rx_packets,
                st.rx_loss,
                st.rx_gaps,
                st.rx_late,
                st.rx_reordered
            ),
            (150, 16, 1, 3, 10)
        );
        let line = st.line();
        assert!(line.contains(" rx_loss=16 rx_gaps=1 rx_late=3 "), "{line}");
        assert!(
            line.ends_with(" rx_reordered=10 start_holes=0 tx_resyncs=0 play_at_steps=0 play_at_max_dev_us=0 stamp_jumps=0 gaps_unfilled=0 tx_late_pushes=0 tx_push_gap_max_us=0 min_lead_us=none late_to_mix=0"),
            "{line}"
        );
    }

    /// obs#47 (dlink v0.6.126, dstack #234): the rings' counts add up; the start burst is apart
    /// from the holes.
    #[test]
    fn the_rings_counts_add_up_and_start_holes_are_not_holes() {
        let mut st = Status::default();
        st.ring(Ring {
            holes: 0,
            written_late: 0,
            start_holes: 3324,
        });
        st.ring(Ring {
            holes: 12,
            written_late: 4,
            start_holes: 100,
        });
        st.ring(Ring::default());
        assert_eq!(
            (st.rx_holes, st.rx_written_late, st.start_holes),
            (12, 4, 3424)
        );
        let line = st.line();
        assert!(line.contains(" rx_holes=12 rx_written_late=4 "), "{line}");
        assert!(
            line.ends_with(" rx_reordered=0 start_holes=3424 tx_resyncs=0 play_at_steps=0 play_at_max_dev_us=0 stamp_jumps=0 gaps_unfilled=0 tx_late_pushes=0 tx_push_gap_max_us=0 min_lead_us=none late_to_mix=0"),
            "{line}"
        );
    }

    /// obs#41: the TX's counts land on the line; the ppm rounds to a tenth.
    #[test]
    fn the_tx_counts_land_on_the_line_and_without_tx_they_read_0() {
        let mut st = Status::default();
        assert!(st.line().ends_with(
            "tx_track=0 tx_played=0 tx_underruns=0 tx_recentres=0 tx_overflows=0 tx_ppm=0.0 tx_priming=0 \
             rx_reordered=0 start_holes=0 tx_resyncs=0 play_at_steps=0 play_at_max_dev_us=0 stamp_jumps=0 gaps_unfilled=0 tx_late_pushes=0 tx_push_gap_max_us=0 min_lead_us=none late_to_mix=0"
        ));
        st.tx(crate::tx::TxCounts {
            track: 1,
            played: 4800,
            underruns: 2,
            recentres: 1,
            overflows: 3,
            ppm: 4.26,
            priming: true,
            resyncs: 2,
        });
        assert!(
            st.line().ends_with(
                "tx_track=1 tx_played=4800 tx_underruns=2 tx_recentres=1 tx_overflows=3 tx_ppm=4.3 tx_priming=1 \
                 rx_reordered=0 start_holes=0 tx_resyncs=2 play_at_steps=0 play_at_max_dev_us=0 stamp_jumps=0 gaps_unfilled=0 tx_late_pushes=0 tx_push_gap_max_us=0 min_lead_us=none late_to_mix=0"
            ),
            "{}",
            st.line()
        );
    }

    fn wake(unix_ms: u64, behind_us: u64, rebootstrap: bool) -> LateWake {
        LateWake {
            unix_ms,
            behind_us,
            rebootstrap,
        }
    }

    #[test]
    fn utc_ms_formats_the_epoch_a_leap_day_and_a_soak_time() {
        assert_eq!(utc_ms(0), "1970-01-01T00:00:00.000Z");
        // 2024-02-29 12:34:56.789 UTC
        assert_eq!(utc_ms(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
        // run 43953607643's first late wake: 2026-11-16 21:10:05.648 UTC
        assert_eq!(utc_ms(1_794_863_405_648), "2026-11-16T21:10:05.648Z");
        assert_eq!(utc_ms(1_798_761_599_999), "2026-12-31T23:59:59.999Z");
    }

    #[test]
    fn a_wake_log_logs_each_wake_once() {
        let mut log = WakeLog::default();
        let recent = [wake(2_000, 8_552, true), wake(3_000, 2_700, false)];
        assert_eq!(
            log.lines(1, 2, &recent),
            [
                "dante-input pump: a late wake at 1970-01-01T00:00:02.000Z behind 8552 us \
                 (re-bootstrapped: blocks lost) (obs#71)",
                "dante-input pump: a late wake at 1970-01-01T00:00:03.000Z behind 2700 us \
                 (within the margin) (obs#71)",
            ]
        );
        // the next window with nothing new logs nothing
        assert!(log.lines(1, 2, &recent).is_empty());
        // a new wake after a wall-clock step back is still logged, once
        let stepped = [recent[0], recent[1], wake(1_500, 3_000, false)];
        let lines = log.lines(1, 3, &stepped);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("00:00:01.500Z"), "{lines:?}");
        assert!(log.lines(1, 3, &stepped).is_empty());
    }

    #[test]
    fn a_wake_log_caps_a_window_and_counts_the_rest() {
        let mut log = WakeLog::default();
        let recent: Vec<LateWake> = (1..=20).map(|i| wake(i * 10, 3_000, false)).collect();
        let lines = log.lines(1, 20, &recent);
        assert_eq!(lines.len(), LATE_WAKE_LINES + 1);
        assert!(lines[0].contains("00:00:00.010Z"));
        assert!(lines[LATE_WAKE_LINES - 1].contains("00:00:00.160Z"));
        assert_eq!(
            lines[LATE_WAKE_LINES],
            "dante-input pump: 4 more late wake(s) in this window not listed (obs#71)"
        );
    }

    #[test]
    fn exactly_the_cap_has_no_summary_line() {
        let mut log = WakeLog::default();
        let recent: Vec<LateWake> = (1..=LATE_WAKE_LINES as u64)
            .map(|i| wake(i, 3_000, false))
            .collect();
        assert_eq!(
            log.lines(1, LATE_WAKE_LINES as u64, &recent).len(),
            LATE_WAKE_LINES
        );
    }

    #[test]
    fn a_new_pump_s_count_starts_the_log_again() {
        let mut log = WakeLog::default();
        let old: Vec<LateWake> = (1..=10).map(|i| wake(i * 10, 3_000, false)).collect();
        assert_eq!(log.lines(7, 10, &old).len(), 10);
        // a new pump: its count starts at 0 and passed the old one before this read; 40 wakes,
        // its ring kept the last 32
        let recent: Vec<LateWake> = (9..=40)
            .map(|i| wake(1_000 + i * 10, 3_000, false))
            .collect();
        let lines = log.lines(8, 40, &recent);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("dante-input pump: 8 late wake(s) left")),
            "{lines:?}"
        );
        // the same pump read again: nothing new
        assert!(log.lines(8, 40, &recent).is_empty());
    }

    #[test]
    fn wakes_the_ring_dropped_are_counted_once() {
        let mut log = WakeLog::default();
        // 40 late wakes since the start, the ring kept the last 32 (9..=40)
        let recent: Vec<LateWake> = (9..=40).map(|i| wake(i * 10, 3_000, false)).collect();
        let lines = log.lines(1, 40, &recent);
        assert_eq!(
            lines.last().map(String::as_str),
            Some(
                "dante-input pump: 8 late wake(s) left the pump's last 32 before they were \
                 read; their times are lost (obs#71)"
            )
        );
        // nothing new: no line, the loss is not counted again
        assert!(log.lines(1, 40, &recent).is_empty());
        // a total read before a wake reached the ring is not a loss once it shows
        let mut more = recent.clone();
        more.push(wake(410, 3_000, false));
        assert_eq!(log.lines(1, 41, &more).len(), 1);
    }

    #[test]
    fn the_minute_line_names_the_latest_wake() {
        assert_eq!(
            minute_wake_line(10_445),
            "dante-input pump: the last minute's latest wake 10445 us behind (obs#71)"
        );
    }
}
