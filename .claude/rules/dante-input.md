---
paths:
  - "plugins/dante-input/**"
---

# dante-input tests

- A test that binds fixed UDP/TCP ports keeps every port below Linux's ephemeral range (32768-60999). Ports in that range collide with any outbound socket on the CI runner ("Address already in use"). That failure blocked the v0.4.7 release.
- The device's RX flow ports go up from `rx_port_base`. Choose the base so that base + flows stays below 32768. `tests/device_e2e.rs` uses 22200.

# dante-input latency

- Since v0.4.9 (dlink v0.6.22, dstack #68), `latency_ms` is the budget for each direction's whole path, and dlink splits it. Before, it was the Dante latency alone. The real floor depends on the rates and the block: since dlink v0.6.24 (RX-only device sized by RX work) it is 7 ms at the network/app rate pairs `the_floor_is_7_ms_at_the_tested_rate_pairs` checks. `InputConfig::split` refuses a smaller budget by its key. A dlink pin bump can change the API (latency_ns was removed): run `cargo clippy --all-targets` before pushing.

# dante-input CPUs

- dante-input.toml has no CPU keys. The owner ruled CPU-core pinning for dlink apps out (dstack#1, 4.11.2026). v0.4.11 had `pump_cpu`/`rx_cpu`, and v0.4.12 removed them. Because of `deny_unknown_fields`, a file that still sets them is refused at load.
