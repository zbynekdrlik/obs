//! The `dante-input status:` line, every 10 s (obs#1 log contract; camera-box's E2E program-audio
//! gate reads rx_loss and underruns and wants both at zero). Counters are totals since the start.

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
    pub blocks: u64,
    /// Blocks the pump made late or skipped.
    pub underruns: u64,
    pub overruns: u64,
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
             rx_gaps={} rx_late={} socket_drops={} blocks={} underruns={} overruns={} delivered_frames={}",
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
        if let Some(p) = media.engine() {
            st.media_clock = p.media;
            st.blocks = p.blocks;
            st.underruns = p.late + p.missed;
            st.overruns = p.overruns;
        }
        st
    }
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
            blocks: 50,
            underruns: 5,
            overruns: 6,
            delivered_frames: 4800,
        };
        assert_eq!(
            st.line(),
            "dante-input status: clock=media holdover=0 gm=001dc1fffe0a polls=12 re_anchors=1 \
             rx_packets=100 rx_loss=3 rx_gaps=1 rx_late=2 socket_drops=4 blocks=50 underruns=5 \
             overruns=6 delivered_frames=4800"
        );
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
