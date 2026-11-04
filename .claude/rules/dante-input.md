---
paths:
  - "plugins/dante-input/**"
---

# dante-input tests

- A test that binds fixed UDP/TCP ports keeps every port below Linux's ephemeral range (32768-60999). Ports in that range collide with any outbound socket on the CI runner ("Address already in use"). That failure blocked the v0.4.7 release.
- The device's RX flow ports go up from `rx_port_base`. Choose the base so that base + flows stays below 32768. `tests/device_e2e.rs` uses 22200.
