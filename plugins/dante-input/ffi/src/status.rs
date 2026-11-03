//! The `dante-input status:` line, every 10 s (obs#1 log contract). Counters are totals since the
//! start. Audio reached OBS whole while rx_loss, rx_holes and underruns stay put: rx_loss counts
//! frames the network never brought, rx_holes samples the pump read before their packet came (OBS
//! got silence there), underruns blocks the pump skipped (OBS never got them). late_blocks and
//! overruns are timing only: a block run up to the margin late still reads its own samples.

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
    /// Frames handed to OBS.
    pub delivered_frames: u64,
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
             rx_holes={} rx_written_late={} late_blocks={} rebootstraps={} max_process_us={}",
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
        );
        s
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

    /// The engine pump's counts: only skipped blocks are underruns.
    pub fn pump(&mut self, p: Pump) {
        self.media_clock = p.media;
        self.blocks = p.blocks;
        self.underruns = p.late + p.missed;
        self.late_blocks = p.late;
        self.rebootstraps = p.rebootstraps;
        self.overruns = p.overruns;
        self.max_process_us = p.max_process_us;
    }
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
            delivered_frames: 4800,
        };
        assert_eq!(
            st.line(),
            "dante-input status: clock=media holdover=0 gm=001dc1fffe0a polls=12 re_anchors=1 \
             rx_packets=100 rx_loss=3 rx_gaps=1 rx_late=2 socket_drops=4 blocks=50 underruns=5 \
             overruns=6 delivered_frames=4800 rx_holes=7 rx_written_late=8 late_blocks=9 \
             rebootstraps=10 max_process_us=11"
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
}
