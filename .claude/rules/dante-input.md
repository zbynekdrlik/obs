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
- v0.4.27 appended the TX tokens `tx_track tx_played tx_underruns tx_recentres tx_overflows tx_ppm tx_priming` (obs#41). They read 0 on a device without TX.

# dante-input TX

- `tx` sends one OBS track (`tx_track`, from 1) on the same device (obs#41). OBS's mix runs on OBS's clock and Dante on PTP, so the mix goes through `dlink::bridge`: plugin.c's raw audio callback writes the `Fifo` on OBS's audio thread, and `TxOut::fill` reads it through the ASRC `Player` in the pump. Do not write a ring or a converter of our own.
- plugin.c untaps the raw audio callback (`stop_tx`) before `dante_input_stop`, so no push reaches a stopped device. `test/module_test.c running-tx` checks the order.
- A device with TX splits the budget for both directions: the pump's late margin W drops from the whole budget (RX only) to 2/5 of it. Use latency_ms 20 with TX (W 8 ms; `stream_snv_s_split_with_tx_at_20_ms_is_the_one_dstack_pins`).
- `tests/device_e2e.rs` serializes its devices (`ONE_DEVICE`): two devices in one test process at once would share the PTP follower and ports. Locally run it on a harmless interface (`DANTE_INPUT_TEST_IFACE=docker0 DANTE_INPUT_TEST_IP=172.17.0.1`), never on the Dante VLAN.
