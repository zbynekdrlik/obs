//! The real device on a real interface: start, the device's sockets bound on the config's ports,
//! the status line, stop and the ports free again. Run by CI on a dummy interface it creates
//! (sudo); the interface comes from DANTE_INPUT_TEST_IFACE and the test fails without one.
//!
//! Scope: the device's life, not audio. There is no Dante transmitter on the dummy interface, so
//! nothing here can prove RX audio or loss; camera-box's E2E program-audio gate on strih-snv
//! does, from the `dante-input status:` line (obs#1).

use std::ffi::c_void;
use std::net::UdpSocket;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dante_input_ffi::DanteInput;

/// One device at a time: the tests share the process (and its PTP follower).
static ONE_DEVICE: Mutex<()> = Mutex::new(());

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
    let _one = ONE_DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    let (iface, ip) = iface_ip();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dante-input.toml");
    // Every port sits below Linux's ephemeral range (32768-60999). RX flow ports go up from
    // rx_port_base; 47200 put them where any outbound socket on the CI runner can hold one
    // (v0.4.7 tag run: "RX flow port 47214: Address already in use"). The ARC port 24440 is no
    // device slot's (4440-4471), so both flow bases are set, inside the dstack flow range
    // 29800-31847 (dlink v0.6.118, dstack #264).
    std::fs::write(
        &path,
        format!(
            "name = \"obs-e2e\"\niface = \"{iface}\"\nrx = [\"Master L\", \"Master R\"]\n\
             network_rate = 96000\narc_port = 24440\ncmc_port = 28800\nflows_port = 24455\nrx_port_base = 31000\ntx_port_base = 31032\n\
             [[subscribe]]\nrx = 1\ntx_channel = \"L\"\ntx_device = \"nobody\"\n"
        ),
    )
    .unwrap();
    // OBS at 48 kHz on a 96 kHz network: dlink's 2:1 step.
    let d = DanteInput::start(&path, 48_000, drop_audio, std::ptr::null_mut())
        .expect("the device starts");
    assert_eq!(d.channels(), 2);
    assert_eq!(d.rate(), 48_000);
    for port in [24440, 28800, 24455, 31000, 31032] {
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
        // camera-box#1510 (dstack #234): appended after max_obs_us.
        " wire_late=",
        " wake_delay_us=",
        " wake_delay_peak_us=",
        // obs#41: the TX tokens, 0 on a device without TX.
        " tx_track=0 tx_played=0 tx_underruns=0 tx_recentres=0 tx_overflows=0 tx_ppm=0.0 tx_priming=0",
    ] {
        assert!(line.contains(field), "{field} missing: {line}");
    }
    // obs#45 (dstack #278): reorders apart from loss, the last token, a count.
    let reordered = line
        .rsplit_once(" rx_reordered=")
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("rx_reordered missing: {line}"));
    assert!(
        reordered.parse::<u64>().is_ok(),
        "rx_reordered is the last token, a count: {line}"
    );
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
    for port in [24440, 28800, 24455, 31000, 31032] {
        UdpSocket::bind((ip.as_str(), port))
            .unwrap_or_else(|e| panic!("UDP {ip}:{port} still held after stop: {e}"));
    }
}

/// obs#41 (camera-box#1513): a device with `tx` takes OBS's track as OBS hands it (1024 frames
/// every 1024 frames' time, on OBS's own clock), primes its FIFO and plays it into the TX
/// channels; the status line's tx_* tokens show it. The wire itself is dlink's (dstack's tests).
#[test]
fn a_device_with_tx_plays_obs_s_track_into_its_tx_channels() {
    let _one = ONE_DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    let (iface, _ip) = iface_ip();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dante-input.toml");
    std::fs::write(
        &path,
        format!(
            "name = \"obs-e2e-tx\"\niface = \"{iface}\"\nrx = [\"Master L\", \"Master R\"]\n\
             tx = [\"OBS program L\", \"OBS program R\"]\n\
             network_rate = 96000\narc_port = 24441\ncmc_port = 28801\nflows_port = 24456\nrx_port_base = 31100\ntx_port_base = 31132\n"
        ),
    )
    .unwrap();
    let d = DanteInput::start(&path, 48_000, drop_audio, std::ptr::null_mut())
        .expect("the device with TX starts");
    assert_eq!(d.tx_track(), 1);
    assert_eq!(d.tx_rate(), 48_000);
    let left = vec![0.25f32; 1024];
    let right = vec![-0.25f32; 1024];
    let start = Instant::now();
    let mut sent = 0u64;
    while start.elapsed() < Duration::from_secs(3) {
        // OBS's pace: 1024 frames at 48 kHz every 21.33 ms.
        let due = start.elapsed().as_secs_f64() * 48_000.0;
        while (sent as f64) < due {
            d.tx_push(&[&left, &right], 1024);
            sent += 1024;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(d.ended(), None, "the device ended by itself");
    let st = d.status().expect("a running device has a status");
    let line = st.line();
    eprintln!("{line}");
    assert_eq!(st.tx_track, 1, "{line}");
    assert!(
        !st.tx_priming,
        "the player primed on the pushed track: {line}"
    );
    // ~3 s of pump blocks at 48 kHz, less the start: well past 1 s of frames played.
    assert!(st.tx_played > 48_000, "{line}");
    assert_eq!(st.tx_overflows, 0, "{line}");
    assert!(line.contains(" tx_track=1 tx_played="), "{line}");
    d.stop();
}
