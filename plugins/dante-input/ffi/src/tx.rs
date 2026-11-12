//! The TX of one OBS audio track to Dante (obs#41, camera-box#1513): the stream OBS sends its
//! program mix (track 1) on the device's two TX channels.
//!
//! OBS makes its mix on its own clock (the system clock); the device sends on dlink's media clock
//! (PTP). The two drift, so the mix goes through dlink's bridge (dstack #241, dsend's): OBS's
//! audio thread writes a [`Fifo`] at OBS's rate ([`TxIn::push`]), and the pump reads it through
//! the ASRC [`Player`] at the device's app rate ([`TxOut::fill`]). The player keeps the FIFO at
//! [`player::TARGET_S`] (40 ms) by moving its ratio at most 300 ppm, 5 ppm/s; an underrun plays
//! silence and adds a cushion of up to [`player::CUSHION_MAX_S`]. So OBS's mix reaches the wire
//! the FIFO's depth plus dlink's TX path later: ~60 ms at a 20 ms budget.

use std::sync::Arc;

use dlink::Block;
use dlink::bridge::{Fifo, Player, Stats, fifo_for, player};

use crate::config::TX_CHANNELS;

/// The most frames the pump fills from the player at once (its scratch, no allocation in a block).
pub const CHUNK: usize = 1024;

/// The OBS side: OBS's audio thread writes the track's planes into the FIFO.
pub struct TxIn {
    fifo: Arc<Fifo>,
    stats: Arc<Stats>,
    track: u32,
    rate: u32,
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
    /// The player waits for the FIFO to reach its depth: silence goes out.
    pub priming: bool,
}

/// The TX of OBS's track `track` (from 1): OBS writes at `obs_rate`, the pump reads at `app_rate`.
pub fn tx(track: u32, obs_rate: u32, app_rate: u32) -> Result<(TxIn, TxOut), String> {
    let fifo = Arc::new(fifo_for(TX_CHANNELS, obs_rate));
    let stats = Arc::new(Stats::default());
    let player = Player::new(Arc::clone(&fifo), obs_rate, app_rate, Arc::clone(&stats))
        .map_err(|e| format!("the TX rate conversion {obs_rate} to {app_rate} Hz: {e}"))?;
    Ok((
        TxIn {
            fifo,
            stats,
            track,
            rate: obs_rate,
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

    /// Writes `frames` frames of OBS's planes: the left and right channels, a missing right
    /// (a mono mix) as the left. Frames past the FIFO's room are dropped and counted. Only OBS's
    /// audio thread calls it (the FIFO takes one writer).
    pub fn push(&self, planes: &[&[f32]], frames: usize) {
        let Some(left) = planes.first() else {
            return;
        };
        let right = planes.get(1).unwrap_or(left);
        let frames = frames.min(left.len()).min(right.len());
        self.fifo.push_with(frames, |f, c| {
            f64::from(if c == 0 { left[f] } else { right[f] })
        });
    }

    pub fn counts(&self) -> TxCounts {
        TxCounts {
            track: self.track,
            played: self.stats.played(),
            underruns: self.stats.underruns(),
            recentres: self.stats.recentres(),
            overflows: self.fifo.overflows(),
            ppm: self.stats.ppm(),
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
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(CHUNK);
            let buf = &mut self.scratch[..n * TX_CHANNELS];
            self.player.fill_with(buf, TX_CHANNELS, |x| x);
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

/// The FIFO's depth the player holds, ms (the TX's own latency before dlink's TX path).
pub fn target_ms() -> f64 {
    player::TARGET_S * 1_000.0
}

#[cfg(test)]
mod tests;
