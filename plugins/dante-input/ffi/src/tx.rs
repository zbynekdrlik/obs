//! The TX of one OBS audio track to Dante (obs#41, camera-box#1513): the stream OBS sends its
//! program mix (track 1) on the device's two TX channels.
//!
//! OBS makes its mix on its own clock (the system clock); the device sends on dlink's media clock
//! (PTP). The two drift, so the mix goes through dlink's bridge (dstack #241, dsend's): OBS's
//! audio thread writes a [`Fifo`] at OBS's rate ([`TxIn::push`]), and the pump reads it through
//! the ASRC [`Player`] at the device's app rate ([`TxOut::fill`]). The player keeps the FIFO at the
//! device's app buffer ([`AppBuffer`], dstack #247: OBS's 1024-frame chunk and one pump block,
//! 22.667 ms on a 96 kHz device) by moving its ratio a few ppm. OBS's mix reaches the wire that
//! buffer plus dlink's TX path later; the device logs every term at its start.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use dlink::Block;
use dlink::bridge::{AppBuffer, Fifo, Player, Stats, fifo_for};

use crate::config::TX_CHANNELS;

/// The most frames the pump fills from the player at once (its scratch, no allocation in a block).
pub const CHUNK: usize = 1024;

/// The OBS side: OBS's audio thread writes the track's planes into the FIFO.
pub struct TxIn {
    fifo: Arc<Fifo>,
    stats: Arc<Stats>,
    track: u32,
    rate: u32,
    buffer: AppBuffer,
    cadence: Cadence,
    epoch: Instant,
}

/// obs#56: the TX writer's cadence, OBS's audio thread pushing its chunks. A push more than
/// [`LATE_FACTOR`] times the last push's audio length after it is late: the FIFO (one chunk + one
/// block) runs dry before it, so late pushes tracking `tx_underruns` name the writer, not the
/// reader. One writer (`note`), one reader (`take_gap_max_us`).
#[derive(Debug, Default)]
pub struct Cadence {
    last_ns: AtomicU64,
    last_len_ns: AtomicU64,
    late: AtomicU64,
    gap_max_ns: AtomicU64,
}

/// A push this many times the last push's audio length after it is late (OBS pushes 1024 frames,
/// 21.3 ms at 48 kHz, so 32 ms; the FIFO holds 22.7 ms).
pub const LATE_FACTOR: f64 = 1.5;

impl Cadence {
    /// A push of `frames` at `rate` Hz at `now_ns` (monotonic, > 0).
    pub fn note(&self, now_ns: u64, frames: usize, rate: u32) {
        let last = self.last_ns.swap(now_ns, Ordering::Relaxed);
        if last != 0 && now_ns > last {
            let gap = now_ns - last;
            self.gap_max_ns.fetch_max(gap, Ordering::Relaxed);
            let len = self.last_len_ns.load(Ordering::Relaxed);
            if len > 0 && gap as f64 > LATE_FACTOR * len as f64 {
                self.late.fetch_add(1, Ordering::Relaxed);
            }
        }
        if rate > 0 {
            let len = frames as u64 * 1_000_000_000 / u64::from(rate);
            self.last_len_ns.store(len, Ordering::Relaxed);
        }
    }

    /// Pushes later than [`LATE_FACTOR`] times the last one's length, since the start.
    pub fn late(&self) -> u64 {
        self.late.load(Ordering::Relaxed)
    }

    /// The largest gap between two pushes since the last take, µs (0: under two pushes since).
    /// `take` starts the next window (the status line's interval); a peek leaves it.
    pub fn gap_max_us(&self, take: bool) -> u64 {
        let ns = if take {
            self.gap_max_ns.swap(0, Ordering::Relaxed)
        } else {
            self.gap_max_ns.load(Ordering::Relaxed)
        };
        ns / 1_000
    }
}

/// The pump side: fills the device's TX outputs from the player.
pub struct TxOut {
    player: Player,
    scratch: Vec<f64>,
}

/// The TX's counts, for the status line (obs#41).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TxCounts {
    /// The OBS track carried, from 1; 0 without TX.
    pub track: u32,
    /// Frames the player gave the device (silence while it primes or underran included).
    pub played: u64,
    /// Blocks the player had no input for: silence went out.
    pub underruns: u64,
    /// Depths the player could not steer back and re-centred (a skip or a refill).
    pub recentres: u64,
    /// OBS frames the FIFO had no room for (the pump not reading).
    pub overflows: u64,
    /// The converter's correction, ppm.
    pub ppm: f64,
    /// Blocks that did not start where the last one ended: the player re-primed to its target
    /// (dlink v0.6.136, dstack#303, obs#56).
    pub resyncs: u64,
    /// The player waits for the FIFO to reach its depth: silence goes out.
    pub priming: bool,
}

/// The TX of OBS's track `track` (from 1): OBS writes at `obs_rate`, the pump reads at `app_rate`,
/// the FIFO held at `buffer` (the device's `bridge_buffer`).
pub fn tx(
    track: u32,
    obs_rate: u32,
    app_rate: u32,
    buffer: AppBuffer,
) -> Result<(TxIn, TxOut), String> {
    let fifo = Arc::new(fifo_for(TX_CHANNELS, obs_rate));
    let stats = Arc::new(Stats::default());
    let player = Player::new(
        Arc::clone(&fifo),
        obs_rate,
        app_rate,
        buffer,
        Arc::clone(&stats),
    )
    .map_err(|e| format!("the TX rate conversion {obs_rate} to {app_rate} Hz: {e}"))?;
    Ok((
        TxIn {
            fifo,
            stats,
            track,
            rate: obs_rate,
            buffer,
            cadence: Cadence::default(),
            epoch: Instant::now(),
        },
        TxOut {
            player,
            scratch: vec![0.0; CHUNK * TX_CHANNELS],
        },
    ))
}

impl TxIn {
    /// The OBS track carried, from 1.
    pub fn track(&self) -> u32 {
        self.track
    }

    /// The rate OBS writes at, Hz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The FIFO's depth the player holds, ms (the app buffer, before dlink's TX path).
    pub fn target_ms(&self) -> f64 {
        self.buffer.target_ns() as f64 / 1_000_000.0
    }

    /// Writes `frames` frames of OBS's planes: the left and right channels, a missing right
    /// (a mono mix) as the left. Frames past the FIFO's room are dropped and counted. Only OBS's
    /// audio thread calls it (the FIFO takes one writer).
    pub fn push(&self, planes: &[&[f32]], frames: usize) {
        let Some(left) = planes.first() else {
            return;
        };
        let right = planes.get(1).unwrap_or(left);
        let frames = frames.min(left.len()).min(right.len());
        // + 1: a push in the epoch's first nanosecond is not "no push yet".
        let now_ns = self.epoch.elapsed().as_nanos() as u64 + 1;
        self.cadence.note(now_ns, frames, self.rate);
        self.fifo.push_with(frames, |f, c| {
            f64::from(if c == 0 { left[f] } else { right[f] })
        });
    }

    /// The cadence counts (obs#56): the late pushes since the start, and the largest push gap
    /// since the status line's last take, µs (only the status line takes).
    pub fn cadence(&self, take: bool) -> (u64, u64) {
        (self.cadence.late(), self.cadence.gap_max_us(take))
    }

    pub fn counts(&self) -> TxCounts {
        TxCounts {
            track: self.track,
            played: self.stats.played(),
            underruns: self.stats.underruns(),
            recentres: self.stats.recentres(),
            overflows: self.fifo.overflows(),
            ppm: self.stats.ppm(),
            resyncs: self.stats.resyncs(),
            priming: self.stats.priming(),
        }
    }
}

impl TxOut {
    /// Fills the block's TX outputs (the first [`TX_CHANNELS`]) from the player. No lock, no
    /// allocation, no system call: it runs in the pump's block.
    pub fn fill(&mut self, block: &mut Block<'_>) {
        let frames = block.frames();
        let outputs = block.outputs().min(TX_CHANNELS);
        let start = block.start();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(CHUNK);
            let buf = &mut self.scratch[..n * TX_CHANNELS];
            // obs#56 (dlink v0.6.136): each piece's start, so a block off the last one's end
            // (a rebootstrap's skip) resyncs the player instead of leaving its FIFO off target.
            let at = start.map(|s| s + done as u64);
            self.player.fill_at(at, buf, TX_CHANNELS, |x| x);
            for c in 0..outputs {
                let out = block.output(c);
                for (f, slot) in out.iter_mut().skip(done).take(n).enumerate() {
                    *slot = buf[f * TX_CHANNELS + c];
                }
            }
            done += n;
        }
    }
}

#[cfg(test)]
mod tests;
