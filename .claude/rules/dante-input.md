---
paths:
  - "plugins/dante-input/**"
---

# dante-input tests

- A test that binds fixed UDP/TCP ports keeps every port below Linux's ephemeral range (32768-60999). Ports in that range collide with any outbound socket on the CI runner ("Address already in use"). That failure blocked the v0.4.7 release.
- The device's RX flow ports go up from `rx_port_base`. Choose the base so that base + flows stays below 32768. `tests/device_e2e.rs` uses 22200.

# dante-input latency

- Since v0.4.9 (dlink v0.6.22, dstack #68), `latency_ms` is the budget for each direction's whole path, and dlink splits it. Before, it was the Dante latency alone. The real floor depends on the rates and the block. An RX-only device is sized by RX work alone (dlink v0.6.24). Since v0.6.28 it is 6 ms at 96/48 and 7 ms at the other pairs `the_floor_at_the_tested_rate_pairs` checks. The default follows `dlink::BUDGET_NS` (15 ms since v0.6.28, owner 4.11.2026); a dlink default change moves dante-input's with it, so re-read the dstack CHANGELOG table and the pinned test numbers. `InputConfig::split` refuses a smaller budget by its key. A dlink pin bump can change the API (latency_ns was removed): run `cargo clippy --all-targets` before pushing.

# dante-input CPUs

- dante-input.toml has no CPU keys. The owner ruled CPU-core pinning for dlink apps out (dstack#1, 4.11.2026). v0.4.11 had `pump_cpu`/`rx_cpu`, and v0.4.12 removed them. Because of `deny_unknown_fields`, a file that still sets them is refused at load.

# dante-input status line

- camera-box's E2E program-audio gate parses the `dante-input status:` line by token name. A new token goes at the END, after the last one (v0.4.26 appended `wire_late wake_delay_us wake_delay_peak_us` after `max_obs_us`). Never reorder or rename a token. `the_line_carries_every_contract_field_in_order` pins the whole line, and `tests/device_e2e.rs` checks the tokens on a real device.
- dlink's per-flow reads are per slot (`media.rx.lane(slot)` for 0..32). The line sums the counts and takes the worst slot for delays (`Status::flow`). A `last_*` value is dlink's last heartbeat second, not the print interval.
