//! The TX of one OBS audio track to Dante (obs#41, camera-box#1513): the stream OBS sends its
//! program mix (track 1) on the device's two TX channels.
//!
//! OBS makes its mix on its own clock (the system clock); the device sends on dlink's media clock
//! (PTP). The two drift, so the mix goes through dlink's bridge (dstack #241, dsend's): OBS's
//! audio thread writes a [`Fifo`] at OBS's rate ([`TxIn::push`]), and the device's TX thread reads
//! it through the ASRC [`Player`] at the device's app rate (dlink's [`TxPull`], obs#76: the TX
//! thread fills each block at the pump's own due time and stamp just before it sends, so a late
//! pump no longer costs TX audio, dstack#363). The player keeps the FIFO at the
//! device's app buffer ([`AppBuffer`], dstack #247: OBS's 1024-frame chunk and one pump block,
//! 22.667 ms on a 96 kHz device) by moving its ratio a few ppm. OBS's mix reaches the wire that
//! buffer plus dlink's TX path later; the device logs every term at its start.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use dlink::bridge::{AppBuffer, Fifo, Player, Stats, TxPull, chunk_ns, fifo_for};

use crate::config::{OBS_CHUNK_FRAMES, TX_CHANNELS};

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
            let len = (frames as u128 * 1_000_000_000 / u128::from(rate)) as u64;
            self.last_len_ns.store(len, Ordering::Relaxed);
        }
    }

    /// Pushes later than [`LATE_FACTOR`] times the last one's length, since the start.
    pub fn late(&self) -> u64 {
        self.late.load(Ordering::Relaxed)
    }

    /// The largest gap between two pushes since the last take, µs, the gap still open at `now_ns`
    /// (since the last push) included, so a writer stalled right now shows before its next push
    /// (0: no push yet). `take` starts the next window (the status line's interval); a peek leaves
    /// it. Only a push counts a late one (once).
    pub fn gap_max_us(&self, take: bool, now_ns: u64) -> u64 {
        let closed = if take {
            self.gap_max_ns.swap(0, Ordering::Relaxed)
        } else {
            self.gap_max_ns.load(Ordering::Relaxed)
        };
        let last = self.last_ns.load(Ordering::Relaxed);
        let open = if last != 0 {
            now_ns.saturating_sub(last)
        } else {
            0
        };
        closed.max(open) / 1_000
    }
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
    /// OBS frames the FIFO had no room for (the TX thread not reading).
    pub overflows: u64,
    /// The converter's correction, ppm.
    pub ppm: f64,
    /// Blocks that did not start where the last one ended: the player re-primed to its target
    /// (dlink v0.6.136, dstack#303, obs#56).
    pub resyncs: u64,
    /// The player waits for the FIFO to reach its depth: silence goes out.
    pub priming: bool,
}

/// The TX of OBS's track `track` (from 1): OBS writes at `obs_rate`, the device's TX thread reads
/// at `app_rate` through the returned [`TxPull`] (`Link::start_tx_pulling`), the FIFO held at
/// `buffer` (the device's `bridge_buffer`).
pub fn tx(
    track: u32,
    obs_rate: u32,
    app_rate: u32,
    buffer: AppBuffer,
) -> Result<(TxIn, TxPull), String> {
    tx_on(
        fifo_for(TX_CHANNELS, obs_rate),
        track,
        obs_rate,
        app_rate,
        buffer,
    )
}

/// [`tx`] on a FIFO the caller made: a simulation's FIFO stamps its writes on the simulated
/// clock (`Fifo::with_clock`), since dlink v0.6.143's player steers on the line through the
/// writes' stamps (dstack#303, obs#56).
fn tx_on(
    fifo: Fifo,
    track: u32,
    obs_rate: u32,
    app_rate: u32,
    buffer: AppBuffer,
) -> Result<(TxIn, TxPull), String> {
    let fifo = Arc::new(fifo);
    let stats = Arc::new(Stats::default());
    let player = Player::new(
        Arc::clone(&fifo),
        obs_rate,
        app_rate,
        buffer,
        // OBS writes 1024-frame chunks (dlink v0.6.144, dstack #317: the player follows the
        // producer's real chunk).
        chunk_ns(OBS_CHUNK_FRAMES, obs_rate),
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
        TxPull::new(player, TX_CHANNELS),
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
        let now_ns = self.epoch.elapsed().as_nanos() as u64 + 1;
        (self.cadence.late(), self.cadence.gap_max_us(take, now_ns))
    }

    /// obs#80: OBS frames pushed before the TX thread's first pull (not written, no overflow).
    pub fn unpulled(&self) -> u64 {
        0
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

#[cfg(test)]
mod tests;
