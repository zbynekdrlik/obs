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
fn a_full_file_reads_with_10_ms_and_the_state_file_next_to_it() {
    let (_d, path) = write(GOOD);
    let cfg = InputConfig::load(&path).unwrap();
    assert_eq!(cfg.name, "strih-obs");
    assert_eq!(cfg.iface, "enp7s0.70");
    assert_eq!(cfg.latency_ms, 10);
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
    // dstack #68: 10 ms is the whole path. At 48 kHz, block 128 (2.67 ms) leaves 7.33 ms of
    // Dante latency, in whole samples (352).
    assert_eq!(dev.latency_ns, 352 * 1_000_000_000 / 48_000);
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
fn a_latency_from_the_file_reaches_the_device() {
    let (_d, path) = write(&GOOD.replace(
        "rx_port_base = 47200",
        "rx_port_base = 47200\nlatency_ms = 20",
    ));
    let dev = InputConfig::load(&path)
        .unwrap()
        .dlink(48_000)
        .device()
        .unwrap();
    // 20 ms budget less the 2.67 ms block: 832 samples of Dante latency.
    assert_eq!(dev.latency_ns, 832 * 1_000_000_000 / 48_000);
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
fn a_latency_outside_4_to_100_ms_is_refused() {
    for bad in [0, 3, 101] {
        let (_d, path) = write(&GOOD.replace(
            "rx_port_base = 47200",
            &format!("rx_port_base = 47200\nlatency_ms = {bad}"),
        ));
        let e = InputConfig::load(&path).unwrap_err();
        assert!(
            e.contains(&format!("latency_ms: {bad} is not one of 4..=100")),
            "{e}"
        );
    }
    for good in [4, 100] {
        let (_d, path) = write(&GOOD.replace(
            "rx_port_base = 47200",
            &format!("rx_port_base = 47200\nlatency_ms = {good}"),
        ));
        assert_eq!(InputConfig::load(&path).unwrap().latency_ms, good);
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
fn a_budget_the_work_leaves_under_4_ms_of_is_refused_by_its_key() {
    // dlink v0.6.24: an RX-only device's work is the RX side alone. At a 96 kHz network with OBS
    // at 48 kHz that is the 1.33 ms block and the converter's 1.33 ms RX delay, so 6 ms leaves
    // 3.33 ms and 7 ms leaves 4.33 ms; the RX path is the whole budget.
    let net = |ms: u32| {
        GOOD.replace(
            "rx_port_base = 47200",
            &format!("rx_port_base = 47200\nnetwork_rate = 96000\nlatency_ms = {ms}"),
        )
    };
    let (_d, path) = write(&net(6));
    let e = InputConfig::load(&path).unwrap().split(48_000).unwrap_err();
    assert!(e.starts_with("latency_ms: 6 is too small here: "), "{e}");
    let (_d, path) = write(&net(7));
    let split = InputConfig::load(&path).unwrap().split(48_000).unwrap();
    assert!(split.latency_ns >= 4_000_000, "{split:?}");
    assert!(split.rx_path_ns <= 7_000_000, "{split:?}");
}
