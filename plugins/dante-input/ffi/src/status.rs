//! The `dante-input status:` line, every 1 s for the first 30 s, then every 10 s (obs#1 log contract). Counters are totals since the
//! start. Audio reached OBS whole while rx_loss, rx_holes and underruns stay put: rx_loss counts
//! frames the network never brought, rx_holes samples the pump read before their packet came (OBS
//! got silence there), underruns blocks the pump skipped (OBS never got them). late_blocks and
//! overruns are timing only: a block run up to the margin late still reads its own samples.
//! wire_late, wake_delay_us and wake_delay_peak_us (dlink v0.6.101, dstack #234) tell a late
//! packet late on the wire from our RX thread's late wake; they come last on the line, so the
//! tokens before them keep their order (camera-box#1510). The wake delay is the RX pass's time less
//! the kernel's receive stamp, which is wall clock (CLOCK_REALTIME): a forward step of it between a
//! receive and the pass reads as a false wake delay, and stays in wake_delay_peak_us for the run.
//! Since dlink v0.6.125 (dstack #278) a reordered packet fills the hole its overtaker opened and
//! counts in rx_reordered, last on the line (obs#45); rx_gaps / rx_loss count only the holes still
//! open at playout. The wake delay's minute percentiles are not on the line: dlink logs them once a
//! minute per flow (`RX flow timing in the last minute ...`, dstack #234), through our logger.

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
             tx_priming={} rx_reordered={}",
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
            st.rx_packets += l.packets;
            st.rx_loss += l.gap_frames;
            st.rx_gaps += l.gaps;
            st.rx_late += l.late;
            st.rx_reordered += l.reordered;
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
            st.rx_holes += c.holes;
            st.rx_written_late += c.late;
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
            });
        }
        st
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
    }
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
        };
        assert_eq!(
            st.line(),
            "dante-input status: clock=media holdover=0 gm=001dc1fffe0a polls=12 re_anchors=1 \
             rx_packets=100 rx_loss=3 rx_gaps=1 rx_late=2 socket_drops=4 blocks=50 underruns=5 \
             overruns=6 delivered_frames=4800 rx_holes=7 rx_written_late=8 late_blocks=9 \
             rebootstraps=10 max_process_us=11 obs_dropped=12 max_obs_us=13 \
             wire_late=14 wake_delay_us=15 wake_delay_peak_us=16 \
             tx_track=1 tx_played=96000 tx_underruns=17 tx_recentres=18 tx_overflows=19 \
             tx_ppm=-12.5 tx_priming=0 rx_reordered=20"
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
                 tx_track=0 tx_played=0 tx_underruns=0 tx_recentres=0 tx_overflows=0 tx_ppm=0.0 tx_priming=0 rx_reordered=0"
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
        });
        assert_eq!(st.underruns, 0);
        assert_eq!(st.late_blocks, 28_036);
        st.pump(Pump {
            missed: 3,
            rebootstraps: 1,
            ..Pump::default()
        });
        assert_eq!((st.underruns, st.rebootstraps, st.late_blocks), (3, 1, 0));
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

    /// obs#41: the TX's counts land on the line; the ppm rounds to a tenth.
    #[test]
    fn the_tx_counts_land_on_the_line_and_without_tx_they_read_0() {
        let mut st = Status::default();
        assert!(st.line().ends_with(
            "tx_track=0 tx_played=0 tx_underruns=0 tx_recentres=0 tx_overflows=0 tx_ppm=0.0 tx_priming=0 \
             rx_reordered=0"
        ));
        st.tx(crate::tx::TxCounts {
            track: 1,
            played: 4800,
            underruns: 2,
            recentres: 1,
            overflows: 3,
            ppm: 4.26,
            priming: true,
        });
        assert!(
            st.line().ends_with(
                "tx_track=1 tx_played=4800 tx_underruns=2 tx_recentres=1 tx_overflows=3 tx_ppm=4.3 tx_priming=1 rx_reordered=0"
            ),
            "{}",
            st.line()
        );
    }
}
