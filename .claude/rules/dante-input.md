---
paths:
  - "plugins/dante-input/**"
---

# dante-input tests

- A test that binds fixed UDP/TCP ports keeps every port below Linux's ephemeral range (32768-60999). Ports in that range collide with any outbound socket on the CI runner ("Address already in use"). That failure blocked the v0.4.7 release.
- Since dlink v0.6.118 (dstack #264) the flow ports default to the device's slot block in 29800-31847. A device whose ARC port is no slot's (4440-4471) must set both `rx_port_base` and `tx_port_base`, or it does not start. The tests use ARC 24440/24441, so they set both inside 29800-31847, below the ephemeral range (`tests/device_e2e.rs` 31000/31032 and 31100/31132, the CI module test 31200/31232). Deployed configs leave both unset.

# dante-input latency

- Since v0.4.27 (dlink v0.6.106, dstack #244) the latency is named terms: `network_latency_ms` (a) + `receive_buffer_ms` (b) = the Dante latency P, plus each direction's app hand-off (c). `latency_ms` is gone and refused (strict file). Neither term set keeps P as before (13.041666 ms at 96/48 RX only). `InputConfig::split` maps dlink's ns refusal to the file's keys. obs never states latency numbers itself: quote dlink's own start-log lines (`dante-input dlink: ... dlink latency: ...`) or dstack. A dlink pin bump can break the API (budget_ns was removed): run `cargo clippy --all-targets` before pushing.

# dante-input CPUs

- dante-input.toml has no CPU keys. The owner ruled CPU-core pinning for dlink apps out (dstack#1, 4.11.2026). v0.4.11 had `pump_cpu`/`rx_cpu`, and v0.4.12 removed them. Because of `deny_unknown_fields`, a file that still sets them is refused at load.

# dante-input status line

- camera-box's E2E program-audio gate parses the `dante-input status:` line by token name. A new token goes at the END, after the last one (v0.4.26 appended `wire_late wake_delay_us wake_delay_peak_us` after `max_obs_us`). Never reorder or rename a token. `the_line_carries_every_contract_field_in_order` pins the whole line, and `tests/device_e2e.rs` checks the tokens on a real device.
- dlink's per-flow reads are per slot (`media.rx.lane(slot)` for 0..32). The line sums the counts and takes the worst slot for delays (`Status::flow`). A `last_*` value is dlink's last heartbeat second, not the print interval.
- v0.4.27 appended the TX tokens `tx_track tx_played tx_underruns tx_recentres tx_overflows tx_ppm tx_priming` (obs#41). They read 0 on a device without TX.
- v0.4.29 appended `rx_reordered` (obs#45, dlink v0.6.125, dstack #278): packets that filled a hole another packet opened. Since that pin `rx_gaps`/`rx_loss` count only holes still open at playout, so a reorder is no longer loss. dlink logs the wake delay's minute p50/p99/p99.9/max and the network latency itself (`RX flow timing in the last minute`); they are not tokens.

# dante-input TX

- `tx` sends one OBS track (`tx_track`, from 1) on the same device (obs#41). OBS's mix runs on OBS's clock and Dante on PTP, so the mix goes through `dlink::bridge`: plugin.c's raw audio callback writes the `Fifo` on OBS's audio thread, and `TxOut::fill` reads it through the ASRC `Player` in the pump. Do not write a ring or a converter of our own.
- plugin.c untaps the raw audio callback (`stop_tx`) before `dante_input_stop`, so no push reaches a stopped device. `test/module_test.c running-tx` checks the order.
- With TX, the TX path is the TX hand-off (W + converter) + P; W is dlink's `WORK_MARGIN_NS` unless set. The TX bridge's FIFO is the app buffer (dlink v0.6.112, dstack #247): `Config::app_buffer(chunk_ns(OBS_CHUNK_FRAMES, obs rate), None)` in `InputConfig::dlink`, and `bridge_buffer()` goes to `Player::new`. That is OBS's 1024-frame chunk + one pump block, 22.667 ms on a 96 kHz device. `Split::tx_path_ns` excludes it, so OBS to the wire = tx_path + app buffer (the start line's `obs_to_wire_ns`).
- `tests/device_e2e.rs` serializes its devices (`ONE_DEVICE`): two devices in one test process at once would share the PTP follower and ports. Locally run it on a harmless interface (`DANTE_INPUT_TEST_IFACE=docker0 DANTE_INPUT_TEST_IP=172.17.0.1`), never on the Dante VLAN.
