//! The box's Dante input config: `~/.camera-box/dante-input.toml`, written by camera-box's deploy
//! (obs#1). obs owns the format. No file means no device and no source.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The device latency unless the file sets one (owner, camera-box#1485: 10 ms).
pub const DEFAULT_LATENCY_MS: u32 = 10;
/// dlink's least device latency.
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
    #[serde(default = "default_latency")]
    pub latency_ms: u32,
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

    /// The dlink config of this file: RX only, no TX channels.
    pub fn dlink(&self) -> dlink::Config {
        let state = self.state_file.clone().unwrap_or_default();
        let mut c = dlink::Config::new(self.name.clone(), state)
            .rx(self.rx.clone())
            .latency_ns(u64::from(self.latency_ms) * 1_000_000)
            .product("obs", "dante-input");
        if let (Some(arc), Some(cmc), Some(flows)) = (self.arc_port, self.cmc_port, self.flows_port)
        {
            c = c.ports(dlink::Ports { arc, cmc, flows });
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
