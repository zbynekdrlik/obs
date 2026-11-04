//! The real device on a real interface: start, the device's sockets bound on the config's ports,
//! the status line, stop and the ports free again. Run by CI on a dummy interface it creates
//! (sudo); the interface comes from DANTE_INPUT_TEST_IFACE and the test fails without one.
//!
//! Scope: the device's life, not audio. There is no Dante transmitter on the dummy interface, so
//! nothing here can prove RX audio or loss; camera-box's E2E program-audio gate on strih-snv
//! does, from the `dante-input status:` line (obs#1).

use std::ffi::c_void;
use std::net::UdpSocket;
use std::time::Duration;

use dante_input_ffi::DanteInput;

unsafe extern "C" fn drop_audio(_: *mut c_void, _: *const *const f32, _: u32, _: u32) {}

fn iface_ip() -> (String, String) {
    let iface = std::env::var("DANTE_INPUT_TEST_IFACE")
        .expect("DANTE_INPUT_TEST_IFACE names the interface this test runs the device on");
    let ip = std::env::var("DANTE_INPUT_TEST_IP")
        .expect("DANTE_INPUT_TEST_IP is that interface's IPv4 address");
    (iface, ip)
}

#[test]
fn the_device_starts_on_its_ports_reports_status_and_frees_them_on_stop() {
    let (iface, ip) = iface_ip();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dante-input.toml");
    // Every port sits below Linux's ephemeral range (32768-60999). RX flow ports go up from
    // rx_port_base; 47200 put them where any outbound socket on the CI runner can hold one
    // (v0.4.7 tag run: "RX flow port 47214: Address already in use").
    std::fs::write(
        &path,
        format!(
            "name = \"obs-e2e\"\niface = \"{iface}\"\nrx = [\"Master L\", \"Master R\"]\n\
             network_rate = 96000\narc_port = 24440\ncmc_port = 28800\nflows_port = 24455\nrx_port_base = 22200\n\
             [[subscribe]]\nrx = 1\ntx_channel = \"L\"\ntx_device = \"nobody\"\n"
        ),
    )
    .unwrap();
    // OBS at 48 kHz on a 96 kHz network: dlink's 2:1 step.
    let d = DanteInput::start(&path, 48_000, drop_audio, std::ptr::null_mut())
        .expect("the device starts");
    assert_eq!(d.channels(), 2);
    assert_eq!(d.rate(), 48_000);
    for port in [24440, 28800, 24455, 22200] {
        assert!(
            UdpSocket::bind((ip.as_str(), port)).is_err(),
            "UDP {ip}:{port} should be the device's while it runs"
        );
    }
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(d.ended(), None, "the device ended by itself");
    let line = d.status().expect("a running device has a status").line();
    eprintln!("{line}");
    assert!(line.starts_with("dante-input status: clock="), "{line}");
    for field in [
        " rx_packets=",
        " rx_loss=",
        " underruns=",
        " rx_holes=",
        " late_blocks=",
        " max_process_us=",
        " obs_dropped=",
        " max_obs_us=",
        " delivered_frames=",
    ] {
        assert!(line.contains(field), "{field} missing: {line}");
    }
    // dtray's obs part: dlink calls the hook on its own thread, so the first reads may not
    // carry it yet.
    let mut app = serde_json::Value::Null;
    for _ in 0..40 {
        let v: serde_json::Value =
            serde_json::from_str(&d.device_status().expect("a running device has a status"))
                .expect("dlink's status is JSON");
        app = v["app"].clone();
        if app.is_object() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("app: {app}");
    let state = app["state"].as_str().unwrap_or_default();
    assert!(state == "ok" || state == "warn", "app: {app}");
    assert!(
        app["line"]
            .as_str()
            .unwrap_or_default()
            .starts_with("OBS: "),
        "app: {app}"
    );
    d.stop();
    for port in [24440, 28800, 24455, 22200] {
        UdpSocket::bind((ip.as_str(), port))
            .unwrap_or_else(|e| panic!("UDP {ip}:{port} still held after stop: {e}"));
    }
}
