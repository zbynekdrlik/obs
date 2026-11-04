//! The box's Dante input config: `~/.camera-box/dante-input.toml`, written by camera-box's deploy
//! (obs#1). obs owns the format. No file means no device and no source.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The latency budget unless the file sets one: dlink's own default, 15 ms since dlink v0.6.28
/// (owner ROZHODNUTÉ 4.11.2026, dstack#1; it was 10 ms, camera-box#1485). Since dlink
/// v0.6.22 (dstack #68, owner 3.11.2026) it is each direction's whole path, the Dante latency
/// and the work together; dlink gives the Dante latency what the work of the directions the
/// device uses leaves (v0.6.24: RX only here, so dlink's RX path is the whole budget).
/// Before v0.4.9 the same `latency_ms` was the Dante latency alone.
pub const DEFAULT_LATENCY_MS: u32 = (dlink::BUDGET_NS / 1_000_000) as u32;
/// dlink's least Dante latency, so no budget below it can work. The real floor depends on the
/// rates and the block: [`InputConfig::split`] refuses a budget that leaves under 4 ms after
/// the work (dlink v0.6.28, RX only: 6 ms at 96/48 kHz network/app, 7 ms at 48/48, 88.2/48,
/// 96/44.1, 96/32 and 44.1/44.1 kHz, which the tests check).
pub const MIN_LATENCY_MS: u32 = 4;
/// Past this the program audio is too late to be any use.
pub const MAX_LATENCY_MS: u32 = 100;
/// The most RX channels the source takes: OBS's widest speaker layout (7.1).
pub const MAX_CHANNELS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    /// The RX channel, from 1.
    pub rx: usize,
    pub tx_channel: String,
    pub tx_device: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// The device name Dante Controller lists.
    pub name: String,
    /// The interface that faces the Dante network.
    pub iface: String,
    /// Where the device keeps what Dante Controller sets; next to the config unless set.
    pub state_file: Option<PathBuf>,
    /// The latency budget, ms: each direction's whole path, which dlink splits (dstack #68).
    #[serde(default = "default_latency")]
    pub latency_ms: u32,
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
    pub rx_port_base: Option<u16>,
    pub flow_slots: Option<usize>,
    #[serde(default)]
    pub subscribe: Vec<Subscription>,
}

fn default_latency() -> u32 {
    DEFAULT_LATENCY_MS
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
        if !(MIN_LATENCY_MS..=MAX_LATENCY_MS).contains(&self.latency_ms) {
            problems.push(format!(
                "latency_ms: {} is not one of {MIN_LATENCY_MS}..={MAX_LATENCY_MS}",
                self.latency_ms
            ));
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

    /// How dlink splits this file's budget for OBS at `obs_rate`. A budget that leaves under
    /// 4 ms of Dante latency is refused, named by its key in the file (dlink names it
    /// `budget_ns`); any other refusal passes through as dlink's (a 0 Hz rate, which
    /// [`InputConfig::load`] already refuses by its key).
    pub fn split(&self, obs_rate: u32) -> Result<dlink::Split, String> {
        self.dlink(obs_rate).split().map_err(|e| {
            if e.starts_with("budget_ns:") {
                format!("latency_ms: {} is too small here: {e}", self.latency_ms)
            } else {
                e
            }
        })
    }

    /// The dlink config of this file for OBS running at `obs_rate`: RX only, no TX channels.
    pub fn dlink(&self, obs_rate: u32) -> dlink::Config {
        let state = self.state_file.clone().unwrap_or_default();
        let mut c = dlink::Config::new(self.name.clone(), state)
            .rx(self.rx.clone())
            .budget_ns(u64::from(self.latency_ms) * 1_000_000)
            .sample_rate(self.app_rate(obs_rate))
            .product("obs", "dante-input");
        if let (Some(arc), Some(cmc), Some(flows)) = (self.arc_port, self.cmc_port, self.flows_port)
        {
            c = c.ports(dlink::Ports { arc, cmc, flows });
        }
        if let Some(rate) = self.network_rate {
            c = c.network_rate(rate);
        }
        if let Some(base) = self.rx_port_base {
            c = c.rx_port_base(base);
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

#[cfg(test)]
mod tests;
