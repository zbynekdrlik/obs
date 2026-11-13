use super::*;

const GOOD: &str = r#"
name = "strih-obs"
iface = "enp7s0.70"
rx = ["Master L", "Master R"]
arc_port = 4440
cmc_port = 8800
flows_port = 4455
rx_port_base = 47200

[[subscribe]]
rx = 1
tx_channel = "A"
tx_device = "foh"

[[subscribe]]
rx = 2
tx_channel = "B"
tx_device = "foh"
"#;

fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dante-input.toml");
    std::fs::write(&path, text).unwrap();
    (dir, path)
}

#[test]
fn a_full_file_reads_with_dlink_s_default_latency_terms_and_the_state_file_next_to_it() {
    let (_d, path) = write(GOOD);
    let cfg = InputConfig::load(&path).unwrap();
    assert_eq!(cfg.name, "strih-obs");
    assert_eq!(cfg.iface, "enp7s0.70");
    // dlink v0.6.106 (dstack #244): no term set means dlink's defaults, which keep P as before.
    assert_eq!(cfg.network_latency_ms, None);
    assert_eq!(cfg.receive_buffer_ms, None);
    assert_eq!(cfg.rx, ["Master L", "Master R"]);
    assert_eq!(
        cfg.state_file,
        Some(path.with_file_name("dante-input.state.toml"))
    );
    assert_eq!(cfg.subscribe.len(), 2);
}

#[test]
fn the_dlink_device_takes_ports_subscriptions_latency_and_no_tx() {
    let (_d, path) = write(GOOD);
    let dev = InputConfig::load(&path)
        .unwrap()
        .dlink(48_000)
        .device()
        .unwrap();
    assert_eq!(dev.name, "strih-obs");
    // dlink v0.6.106 (dstack #244): the default terms keep P as before: 592 whole samples at
    // 48 kHz RX only (the old 15 ms budget less the 2.67 ms block).
    assert_eq!(dev.latency_ns, 592 * 1_000_000_000 / 48_000);
    assert_eq!(
        (dev.arc_port, dev.cmc_port, dev.flows_port),
        (4440, 8800, 4455)
    );
    assert_eq!(dev.rx_port_base, Some(47200));
    assert_eq!(dev.rx, ["Master L", "Master R"]);
    assert!(dev.tx.is_empty());
    assert_eq!(dev.subscribe.len(), 2);
}

#[test]
fn the_file_s_latency_terms_reach_the_device() {
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nnetwork_latency_ms = 2\nreceive_buffer_ms = 5",
    ));
    let cfg = InputConfig::load(&path).unwrap();
    let split = cfg.split(48_000).unwrap();
    assert_eq!(split.network_latency_ns, 2_000_000);
    assert_eq!(split.receive_buffer_ns, 5_000_000);
    // P = (a) + (b): 7 ms, 336 whole samples at 48 kHz.
    let dev = cfg.dlink(48_000).device().unwrap();
    assert_eq!(dev.latency_ns, 7_000_000);
}

#[test]
fn a_fraction_of_a_ms_is_a_term_dante_controller_offers() {
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nnetwork_latency_ms = 0.25",
    ));
    let split = InputConfig::load(&path).unwrap().split(48_000).unwrap();
    assert_eq!(split.network_latency_ns, 250_000);
}

#[test]
fn latency_ms_is_gone_and_a_file_that_still_sets_it_is_refused() {
    // dstack #244: dlink has no single budget any more; no deployed file sets it (camera-box,
    // 13.11.2026).
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nlatency_ms = 15",
    ));
    let e = InputConfig::load(&path).unwrap_err();
    assert!(e.contains("unknown field `latency_ms`"), "{e}");
}

#[test]
fn every_problem_is_listed() {
    let bad = r#"
name = "x"
iface = ""
rx = ["1","2","3","4","5","6","7"]
arc_port = 4440
[[subscribe]]
rx = 9
tx_channel = "A"
tx_device = "foh"
"#;
    let (_d, path) = write(bad);
    let e = InputConfig::load(&path).unwrap_err();
    assert!(e.contains("iface: empty"), "{e}");
    assert!(e.contains("rx: 7 channels"), "{e}");
    assert!(e.contains("set all three or none"), "{e}");
    assert!(e.contains("subscribe: rx 9"), "{e}");
}

#[test]
fn an_unknown_key_is_refused() {
    let (_d, path) = write(&format!("{GOOD}\nlatency = 5\n"));
    let e = InputConfig::load(&path).unwrap_err();
    assert!(e.contains("unknown field"), "{e}");
}

#[test]
fn a_missing_file_names_the_path() {
    let e = InputConfig::load(Path::new("/nonexistent/dante-input.toml")).unwrap_err();
    assert!(e.starts_with("/nonexistent/dante-input.toml: "), "{e}");
}

#[test]
fn a_term_of_0_or_less_or_over_100_ms_is_refused_by_its_key() {
    for key in ["network_latency_ms", "receive_buffer_ms"] {
        for bad in ["0", "-1", "100.5"] {
            let (_d, path) = write(&GOOD.replace(
                "rx_port_base = 47200",
                &format!("rx_port_base = 47200\n{key} = {bad}"),
            ));
            let e = InputConfig::load(&path).unwrap_err();
            assert!(
                e.contains(&format!(
                    "{key}: {} is not over 0 and up to 100 ms",
                    bad.parse::<f64>().unwrap()
                )),
                "{e}"
            );
        }
        let (_d, path) = write(&GOOD.replace(
            "rx_port_base = 47200",
            &format!("rx_port_base = 47200\n{key} = 100"),
        ));
        InputConfig::load(&path).unwrap();
    }
}

#[test]
fn obs_gets_its_own_rate_and_the_device_runs_at_the_network_s() {
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nnetwork_rate = 96000",
    ));
    let cfg = InputConfig::load(&path).unwrap();
    assert_eq!(cfg.app_rate(48_000), 48_000);
    let link = cfg.dlink(48_000);
    assert_eq!(link.device_rate(), 96_000);
    assert!(link.converts());
    assert_eq!(link.device().unwrap().sample_rate, 96_000);
}

#[test]
fn without_a_network_rate_the_device_runs_at_obs_s() {
    let (_d, path) = write(GOOD);
    let cfg = InputConfig::load(&path).unwrap();
    assert_eq!(cfg.dlink(48_000).device().unwrap().sample_rate, 48_000);
    assert!(!cfg.dlink(48_000).converts());
    // OBS's rate unknown: dlink's.
    assert_eq!(cfg.app_rate(0), dlink::SAMPLE_RATE);
}

#[test]
fn the_file_s_sample_rate_wins_over_obs_s() {
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nsample_rate = 48000\nnetwork_rate = 96000",
    ));
    let cfg = InputConfig::load(&path).unwrap();
    assert_eq!(cfg.app_rate(44_100), 48_000);
    assert!(cfg.dlink(44_100).converts());
}

/// dlink v0.6.8 (dstack #90): any app rate converts on the device's clock, no longer only half.
#[test]
fn obs_at_44_1_khz_on_a_96_khz_network_runs_through_dlink_s_converter() {
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nnetwork_rate = 96000",
    ));
    let link = InputConfig::load(&path).unwrap().dlink(44_100);
    assert!(link.converts());
    assert_eq!(link.device().unwrap().sample_rate, 96_000);
}

#[test]
fn a_rate_dlink_refuses_is_refused_before_anything_runs() {
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nsample_rate = 12345",
    ));
    let problems = InputConfig::load(&path)
        .unwrap()
        .dlink(48_000)
        .device()
        .unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("sample_rate: 12345")),
        "{problems:?}"
    );
}

#[test]
fn the_example_config_reads_and_subscribes_from_two_transmitters() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dante-input.example.toml");
    let cfg = InputConfig::load(&path).unwrap();
    assert_eq!(cfg.rx.len(), 4);
    assert_eq!(cfg.network_rate, Some(96_000));
    let devices: std::collections::BTreeSet<_> =
        cfg.subscribe.iter().map(|s| s.tx_device.as_str()).collect();
    assert_eq!(devices.len(), 2, "{devices:?}");
    let link = cfg.dlink(48_000);
    assert!(link.converts());
    let dev = link.device().unwrap();
    assert_eq!(dev.subscribe.len(), 4);
    assert_eq!(dev.sample_rate, 96_000);
}

#[test]
fn a_dante_latency_under_dlink_s_floor_is_refused_by_the_file_s_keys() {
    // dlink's floor for an RX-only device is 4 ms (MIN_LATENCY_NS): P = (a) + (b) under it is
    // refused, named by the file's keys.
    let terms = |a: &str, b: &str| {
        GOOD.replace(
            "rx_port_base = 47200",
            &format!(
                "rx_port_base = 47200\nnetwork_rate = 96000\nnetwork_latency_ms = {a}\nreceive_buffer_ms = {b}"
            ),
        )
    };
    let (_d, path) = write(&terms("0.25", "1"));
    let e = InputConfig::load(&path).unwrap().split(48_000).unwrap_err();
    assert!(
        e.starts_with(
            "network_latency_ms + receive_buffer_ms (0.25 ms + 1 ms) are too small here:"
        ),
        "{e}"
    );
    let (_d, path) = write(&terms("1", "3"));
    let split = InputConfig::load(&path).unwrap().split(48_000).unwrap();
    assert_eq!(split.latency_ns, 4_000_000);
}

#[test]
fn strih_snv_s_default_split_is_the_one_dstack_publishes() {
    // camera-box#1485: strih-snv runs a 96 kHz network with OBS at 48 kHz and sets no latency
    // term. P stays 13041666 ns (dstack v0.6.28), now stated as 1 ms network latency + 12.041666
    // ms receive buffer (dstack #244, v0.6.106); the RX path is P + the hand-off, ~15 ms.
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nnetwork_rate = 96000",
    ));
    let split = InputConfig::load(&path).unwrap().split(48_000).unwrap();
    assert_eq!(split.latency_ns, 13_041_666);
    assert_eq!(split.network_latency_ns, 1_000_000);
    assert_eq!(split.receive_buffer_ns, 12_041_666);
    assert!(
        (14_990_000..=15_000_000).contains(&split.rx_path_ns),
        "{split:?}"
    );
}

#[test]
fn a_zero_rate_is_refused_on_load_by_its_key() {
    for key in ["network_rate", "sample_rate"] {
        let (_d, path) = write(&GOOD.replace(
            "rx_port_base = 47200",
            &format!("rx_port_base = 47200\n{key} = 0"),
        ));
        let e = InputConfig::load(&path).unwrap_err();
        assert!(e.contains(&format!("{key}: 0 Hz")), "{e}");
    }
}

const TX: &str = "rx_port_base = 47200\ntx = [\"OBS program L\", \"OBS program R\"]";

#[test]
fn tx_channels_reach_the_device_and_carry_track_1_unless_set() {
    // obs#41 (camera-box#1513): the stream OBS sends its program mix (track 1) on two TX channels.
    let (_d, path) = write(&GOOD.replace("rx_port_base = 47200", TX));
    let cfg = InputConfig::load(&path).unwrap();
    assert_eq!(cfg.tx_track(), 1);
    let dev = cfg.dlink(48_000).device().unwrap();
    assert_eq!(dev.tx, ["OBS program L", "OBS program R"]);
    assert_eq!(dev.rx, ["Master L", "Master R"]);

    let (_d, path) = write(&GOOD.replace("rx_port_base = 47200", &format!("{TX}\ntx_track = 3")));
    assert_eq!(InputConfig::load(&path).unwrap().tx_track(), 3);
}

#[test]
fn without_tx_there_is_no_track() {
    let (_d, path) = write(GOOD);
    assert_eq!(InputConfig::load(&path).unwrap().tx_track(), 0);
}

#[test]
fn a_tx_of_other_than_two_channels_or_a_track_outside_1_to_6_is_refused_by_its_key() {
    for (keys, wanted) in [
        (
            "tx = [\"L\"]",
            "tx: 1 channels; the TX takes 2 (left, right) or none",
        ),
        (
            "tx = [\"L\", \"R\", \"C\"]",
            "tx: 3 channels; the TX takes 2 (left, right) or none",
        ),
        ("tx_track = 1", "tx_track: set without tx"),
        ("tx = [\"L\", \" \"]", "tx: a channel name is empty"),
        ("tx = [\"L\", \"L\"]", "tx: both channels are named \"L\""),
        (
            "tx = [\"L\", \"R\"]\ntx_track = 0",
            "tx_track: 0 is not one of 1..=6",
        ),
        (
            "tx = [\"L\", \"R\"]\ntx_track = 7",
            "tx_track: 7 is not one of 1..=6",
        ),
    ] {
        let (_d, path) = write(&GOOD.replace(
            "rx_port_base = 47200",
            &format!("rx_port_base = 47200\n{keys}"),
        ));
        let err = InputConfig::load(&path).unwrap_err();
        assert!(err.contains(wanted), "{keys}: {err}");
    }
}

#[test]
fn a_device_with_tx_states_its_tx_hand_off_with_dlink_s_work_margin() {
    // dstack #244: with TX the TX path is the TX hand-off (c) (W + the converter) plus P, and W
    // is dlink's WORK_MARGIN_NS unless set. The program TX's own latency comes from dstack #247.
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        &format!("{TX}\nnetwork_rate = 96000"),
    ));
    let split = InputConfig::load(&path).unwrap().split(48_000).unwrap();
    assert_eq!(split.work_margin_ns, dlink::WORK_MARGIN_NS);
    assert_eq!(split.tx_path_ns, split.tx_handoff_ns + split.latency_ns);
    assert_eq!(
        split.tx_handoff_ns,
        split.work_margin_ns + split.tx_convert_ns
    );
}

#[test]
fn the_stream_pc_s_program_tx_latency_is_the_one_dstack_quotes() {
    // dstack #247 / v0.6.112 (peer, 13.11.2026): the stream PC's device with TX, 96 kHz network,
    // OBS at 48 kHz, block 128, as camera-box will configure it (a 1 ms, b 12.041666 ms to keep
    // its RX latency): app buffer 22.667 + W 6 + TX converter 0.635 + P 13.042 = ~42.3 ms.
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        &format!(
            "{TX}\nnetwork_rate = 96000\nnetwork_latency_ms = 1\nreceive_buffer_ms = 12.041666"
        ),
    ));
    let cfg = InputConfig::load(&path).unwrap();
    let split = cfg.split(48_000).unwrap();
    let buffer = cfg.dlink(48_000).bridge_buffer().unwrap();
    assert_eq!(buffer.target_ns(), 22_666_666);
    assert_eq!(split.latency_ns, 13_041_666);
    assert_eq!(split.work_margin_ns, 6_000_000);
    assert_eq!((split.tx_convert_ns + 500) / 1_000, 635);
    let total = split.tx_path_ns + buffer.target_ns();
    assert!(
        (42_300_000..42_400_000).contains(&total),
        "{total} {split:?}"
    );
}

#[test]
fn a_device_without_tx_has_no_app_buffer() {
    let (_d, path) = write(GOOD);
    assert_eq!(
        InputConfig::load(&path)
            .unwrap()
            .dlink(48_000)
            .bridge_buffer(),
        None
    );
}

#[test]
fn on_a_48_khz_device_the_app_buffer_is_obs_s_chunk_and_a_48_khz_block() {
    // dlink sizes the block from the device's rate: 21.333 ms + 128 frames at 48 kHz (2.667 ms)
    // = 24 ms (dstack v0.6.112 CHANGELOG).
    let (_d, path) = write(&GOOD.replace("rx_port_base = 47200", TX));
    let buffer = InputConfig::load(&path)
        .unwrap()
        .dlink(48_000)
        .bridge_buffer()
        .unwrap();
    assert_eq!(buffer.target_ns(), 24_000_000);
}
