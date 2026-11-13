//! The box's Dante input config: `~/.camera-box/dante-input.toml`, written by camera-box's deploy
//! (obs#1). obs owns the format. No file means no device and no source.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The longest latency term the file may set, ms: past it the program audio is too late to be
/// any use.
pub const MAX_TERM_MS: f64 = 100.0;
/// The most RX channels the source takes: OBS's widest speaker layout (7.1).
pub const MAX_CHANNELS: usize = 8;
/// The TX channels: the left and right of one OBS track (obs#41).
pub const TX_CHANNELS: usize = 2;
/// OBS's audio tracks, from 1 (`MAX_AUDIO_MIXES`).
pub const TRACKS: u32 = 6;
/// The frames OBS hands a raw audio callback at once (`AUDIO_OUTPUT_FRAMES`): the TX bridge's
/// producer chunk (dstack #247).
pub const OBS_CHUNK_FRAMES: u64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    /// The RX channel, from 1.
    pub rx: usize,
    pub tx_channel: String,
    pub tx_device: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// The device name Dante Controller lists.
    pub name: String,
    /// The interface that faces the Dante network.
    pub iface: String,
    /// Where the device keeps what Dante Controller sets; next to the config unless set.
    pub state_file: Option<PathBuf>,
    /// (a) The Dante network latency of the device's subscriptions, ms: usually one of Dante
    /// Controller's 0.25, 0.5, 1, 2 or 5, but dlink takes any value and decides; it refuses only
    /// a P = (a) + (b) under its floor. dlink's 1 ms unless set (dlink v0.6.106, dstack #244).
    pub network_latency_ms: Option<f64>,
    /// (b) This PC's receive buffer, ms: the RX thread's room to wake and pass, sized from the
    /// status line's wake delays. Unset, dlink keeps the Dante latency P = (a) + (b) as it was
    /// before v0.6.106 (13.041666 ms at 96/48 kHz RX only). `latency_ms` is gone (dstack #244).
    pub receive_buffer_ms: Option<f64>,
    /// Hz: the rate OBS gets its blocks at. OBS's own audio rate unless set (owner, dstack #90:
    /// obs keeps OBS's rate, dlink converts).
    pub sample_rate: Option<u32>,
    /// Hz: the Dante network's rate when it differs (the site runs 96000). The device runs at it
    /// and dlink's 2:1 step hands OBS half of it; it must be the sample rate or twice it.
    pub network_rate: Option<u32>,
    /// The RX channels' names, in channel order: OBS gets them in this order.
    pub rx: Vec<String>,
    pub arc_port: Option<u16>,
    pub cmc_port: Option<u16>,
    pub flows_port: Option<u16>,
    /// The RX flow ports from this base (32 ports). Unset: the device's slot block in the dstack
    /// flow range 29800-31847 (dlink v0.6.118, dstack #264), which the generic Dante-leg filter
    /// accepts; leave it unset unless the ARC port is no slot's (4440-4471).
    pub rx_port_base: Option<u16>,
    /// The TX flow ports from this base (32 ports). Unset: the slot's TX block. A device whose ARC
    /// port is no slot's (4440-4471) must set both bases (dlink v0.6.118).
    pub tx_port_base: Option<u16>,
    pub flow_slots: Option<usize>,
    #[serde(default)]
    pub subscribe: Vec<Subscription>,
    /// The TX channels' names, left then right: the device sends OBS's track `tx_track` on them
    /// (obs#41, camera-box#1513). Empty: no TX, the device is RX only as before.
    #[serde(default)]
    pub tx: Vec<String>,
    /// The OBS audio track the TX carries, 1 to 6; track 1 unless set. Only with `tx`.
    pub tx_track: Option<u32>,
}

impl InputConfig {
    /// Reads and checks the file at `path`. Every problem is listed.
    pub fn load(path: &Path) -> Result<InputConfig, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut cfg: InputConfig =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if cfg.state_file.is_none() {
            cfg.state_file = Some(path.with_file_name("dante-input.state.toml"));
        }
        cfg.check()
            .map_err(|p| format!("{}: {}", path.display(), p.join("; ")))?;
        Ok(cfg)
    }

    /// The problems of a parsed config that dlink does not check itself.
    fn check(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if self.iface.is_empty() {
            problems.push("iface: empty".to_string());
        }
        for (key, rate) in [
            ("network_rate", self.network_rate),
            ("sample_rate", self.sample_rate),
        ] {
            if rate == Some(0) {
                problems.push(format!("{key}: 0 Hz"));
            }
        }
        for (key, term) in [
            ("network_latency_ms", self.network_latency_ms),
            ("receive_buffer_ms", self.receive_buffer_ms),
        ] {
            if let Some(ms) = term
                && !(ms > 0.0 && ms <= MAX_TERM_MS)
            {
                problems.push(format!(
                    "{key}: {ms} is not over 0 and up to {MAX_TERM_MS} ms"
                ));
            }
        }
        let n = self.rx.len();
        if n == 0 || n > MAX_CHANNELS || n == 7 {
            problems.push(format!("rx: {n} channels; OBS takes 1 to 6 or 8"));
        }
        let ports = [self.arc_port, self.cmc_port, self.flows_port];
        if ports.iter().any(Option::is_some) && !ports.iter().all(Option::is_some) {
            problems.push("arc_port, cmc_port, flows_port: set all three or none".to_string());
        }
        for s in &self.subscribe {
            if s.rx == 0 || s.rx > n {
                problems.push(format!("subscribe: rx {} is not one of 1..={n}", s.rx));
            }
        }
        let t = self.tx.len();
        if self.tx.iter().any(|n| n.trim().is_empty()) {
            problems.push("tx: a channel name is empty".to_string());
        }
        if t == TX_CHANNELS && self.tx[0] == self.tx[1] {
            problems.push(format!("tx: both channels are named {:?}", self.tx[0]));
        }
        if t != 0 && t != TX_CHANNELS {
            problems.push(format!(
                "tx: {t} channels; the TX takes {TX_CHANNELS} (left, right) or none"
            ));
        }
        match self.tx_track {
            Some(_) if self.tx.is_empty() => {
                problems.push("tx_track: set without tx".to_string());
            }
            Some(k) if !(1..=TRACKS).contains(&k) => {
                problems.push(format!("tx_track: {k} is not one of 1..={TRACKS}"));
            }
            _ => {}
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// The rate OBS gets: the file's, else OBS's own (`obs_rate`, 0 when unknown), else dlink's.
    pub fn app_rate(&self, obs_rate: u32) -> u32 {
        match (self.sample_rate, obs_rate) {
            (Some(r), _) => r,
            (None, 0) => dlink::SAMPLE_RATE,
            (None, r) => r,
        }
    }

    /// dlink's latency terms for this file at `obs_rate` (dstack #244). A Dante latency under its
    /// floor is refused named by the file's keys (dlink names them in ns); any other refusal
    /// passes through as dlink's.
    pub fn split(&self, obs_rate: u32) -> Result<dlink::Split, String> {
        self.dlink(obs_rate).split().map_err(|e| {
            match e.strip_prefix("network_latency_ns + receive_buffer_ns:") {
                Some(rest) => format!(
                    "network_latency_ms + receive_buffer_ms ({} + {}) are too small here:{rest}",
                    term_text(self.network_latency_ms),
                    term_text(self.receive_buffer_ms),
                ),
                None => e,
            }
        })
    }

    /// The rate OBS hands the TX track at: OBS's own (`obs_rate`), else the app rate OBS takes.
    pub fn obs_side_rate(&self, obs_rate: u32) -> u32 {
        if obs_rate == 0 {
            self.app_rate(obs_rate)
        } else {
            obs_rate
        }
    }

    /// The OBS track the TX carries, from 1; 0 when the file has no TX.
    pub fn tx_track(&self) -> u32 {
        if self.tx.is_empty() {
            0
        } else {
            self.tx_track.unwrap_or(1)
        }
    }

    /// The dlink config of this file for OBS running at `obs_rate`: its RX channels, and its TX
    /// channels when the file has them.
    pub fn dlink(&self, obs_rate: u32) -> dlink::Config {
        let state = self.state_file.clone().unwrap_or_default();
        let mut c = dlink::Config::new(self.name.clone(), state)
            .rx(self.rx.clone())
            .tx(self.tx.clone())
            .sample_rate(self.app_rate(obs_rate))
            .product("obs", "dante-input");
        if let (Some(arc), Some(cmc), Some(flows)) = (self.arc_port, self.cmc_port, self.flows_port)
        {
            c = c.ports(dlink::Ports { arc, cmc, flows });
        }
        if let Some(ms) = self.network_latency_ms {
            c = c.network_latency_ns(ms_to_ns(ms));
        }
        if let Some(ms) = self.receive_buffer_ms {
            c = c.receive_buffer_ns(ms_to_ns(ms));
        }
        if let Some(rate) = self.network_rate {
            c = c.network_rate(rate);
        }
        if !self.tx.is_empty() {
            // dstack #247: the TX bridge's FIFO holds OBS's chunk and one pump block.
            c = c.app_buffer(
                dlink::bridge::chunk_ns(OBS_CHUNK_FRAMES, self.obs_side_rate(obs_rate)),
                None,
            );
        }
        if let Some(base) = self.rx_port_base {
            c = c.rx_port_base(base);
        }
        if let Some(base) = self.tx_port_base {
            c = c.tx_port_base(base);
        }
        if let Some(n) = self.flow_slots {
            c = c.flow_slots(n);
        }
        for s in &self.subscribe {
            c = c.subscribe(s.rx, s.tx_channel.clone(), s.tx_device.clone());
        }
        c
    }
}

/// A file's ms as dlink's ns, to the nearest ns.
fn ms_to_ns(ms: f64) -> u64 {
    (ms * 1_000_000.0).round() as u64
}

/// A term as the refusal names it: its ms, or "default".
fn term_text(ms: Option<f64>) -> String {
    ms.map_or_else(|| "default".to_string(), |v| format!("{v} ms"))
}

#[cfg(test)]
mod tests;
