use super::*;

use std::sync::Mutex as StdMutex;

/// What the C callback saw: (channels, frames, samples per channel).
type Seen = (u32, u32, Vec<Vec<f32>>);
/// One test's callbacks: the callback's `opaque` points here, so parallel tests do not mix.
type SeenBox = StdMutex<Vec<Seen>>;

unsafe extern "C" fn record(
    opaque: *mut c_void,
    planes: *const *const f32,
    channels: u32,
    frames: u32,
) {
    let mut chans = Vec::new();
    for c in 0..channels as usize {
        // SAFETY: the sink hands `channels` planes of `frames` samples.
        let p = unsafe { *planes.add(c) };
        chans.push(unsafe { std::slice::from_raw_parts(p, frames as usize) }.to_vec());
    }
    // SAFETY: every test passes its own SeenBox, alive for the test.
    unsafe { &*(opaque as *const SeenBox) }
        .lock()
        .unwrap()
        .push((channels, frames, chans));
}

/// A sink and a feeder over a ring of `frames` frames of `channels`.
fn pair(seen: &SeenBox, channels: usize, frames: usize) -> (Sink, Feeder, Arc<Feed>, Arc<Gate>) {
    let feed = Arc::new(Feed::default());
    let gate = Gate::new();
    let (p, c) = ring(channels, frames);
    let sink = Sink::new(p, channels, Arc::clone(&feed));
    let feeder = Feeder::new(
        record,
        (seen as *const SeenBox).cast_mut().cast(),
        c,
        channels,
        Arc::clone(&feed),
        Arc::clone(&gate),
    );
    (sink, feeder, feed, gate)
}

fn push(sink: &mut Sink, frames: usize, input: &[f64]) {
    let mut out: [f64; 0] = [];
    let mut block = Block::new(frames, input, &mut out);
    sink.process(&mut block);
}

#[test]
fn the_pump_side_queues_and_the_feeder_hands_float_planes_to_obs() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 2, 64);
    // Two channels of 4 frames, channel-major.
    let input = [0.5, -0.25, 1.0, 0.0, -1.0, 0.125, 0.75, -0.5];
    push(&mut sink, 4, &input);
    push(&mut sink, 4, &input);
    // Nothing reaches OBS on the pump's thread.
    assert!(seen_box.lock().unwrap().is_empty());
    assert_eq!(feed.delivered.load(Ordering::Relaxed), 0);
    assert_eq!(feeder.drain(), Some(8));
    let seen = seen_box.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let (channels, frames, chans) = &seen[0];
    assert_eq!((*channels, *frames), (2, 8));
    assert_eq!(chans[0], [0.5, -0.25, 1.0, 0.0, 0.5, -0.25, 1.0, 0.0]);
    assert_eq!(chans[1], [-1.0, 0.125, 0.75, -0.5, -1.0, 0.125, 0.75, -0.5]);
    assert_eq!(feed.delivered.load(Ordering::Relaxed), 8);
    assert_eq!(feed.dropped.load(Ordering::Relaxed), 0);
    assert_eq!(feeder.drain(), Some(0), "the ring is empty after a drain");
}

#[test]
fn the_feeder_hands_at_most_a_chunk_per_call_and_keeps_the_order() {
    let seen_box = SeenBox::default();
    let total = MAX_CHUNK * 2 + 17;
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, total);
    let input: Vec<f64> = (0..total).map(|i| i as f64 / total as f64).collect();
    push(&mut sink, total, &input);
    assert_eq!(feeder.drain(), Some(total));
    let seen = seen_box.lock().unwrap().clone();
    let sizes: Vec<u32> = seen.iter().map(|s| s.1).collect();
    assert_eq!(sizes, [MAX_CHUNK as u32, MAX_CHUNK as u32, 17]);
    let joined: Vec<f32> = seen.iter().flat_map(|s| s.2[0].clone()).collect();
    let want: Vec<f32> = input.iter().map(|&x| x as f32).collect();
    assert_eq!(joined, want);
    assert_eq!(feed.delivered.load(Ordering::Relaxed), total as u64);
}

#[test]
fn a_full_ring_drops_the_block_and_counts_it_without_blocking_the_pump() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 2, 6);
    let input = [0.0; 8];
    push(&mut sink, 4, &input);
    // 4 of 6 frames used: the next 4 do not fit.
    push(&mut sink, 4, &input);
    assert_eq!(feed.dropped.load(Ordering::Relaxed), 4);
    assert_eq!(feeder.drain(), Some(4));
}

#[test]
fn a_closed_gate_lets_nothing_reach_obs() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, gate) = pair(&seen_box, 1, 16);
    push(&mut sink, 4, &[0.1, 0.2, 0.3, 0.4]);
    assert!(gate.close(Duration::from_millis(10)));
    assert_eq!(feeder.drain(), None);
    assert!(seen_box.lock().unwrap().is_empty());
    assert_eq!(feed.delivered.load(Ordering::Relaxed), 0);
}

#[test]
fn a_bad_config_does_not_start_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dante-input.toml");
    std::fs::write(&path, "name = \"x\"\n").unwrap();
    let mut err = [0 as c_char; 256];
    let p = CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: valid path, buffer and callback.
    let d = unsafe {
        dante_input_start(
            p.as_ptr(),
            48_000,
            record,
            std::ptr::null_mut(),
            err.as_mut_ptr(),
            err.len(),
        )
    };
    assert!(d.is_null());
    // SAFETY: write_err NUL-terminates inside the buffer.
    let why = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy();
    assert!(why.contains("missing field"), "{why}");
}

#[test]
fn an_error_longer_than_the_buffer_is_cut_and_terminated() {
    let mut err = [1 as c_char; 8];
    write_err(err.as_mut_ptr(), err.len(), "0123456789");
    // SAFETY: terminated inside the buffer.
    assert_eq!(
        unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap(),
        "0123456"
    );
}

#[test]
fn no_device_has_no_rate() {
    // SAFETY: NULL is allowed.
    assert_eq!(unsafe { dante_input_sample_rate(std::ptr::null()) }, 0);
}

#[test]
fn closing_waits_for_a_callback_in_flight_and_gives_up_after_the_wait() {
    let gate = Gate::new();
    let pass = gate.enter().expect("open");
    // A callback still inside: the close gives up after its wait.
    assert!(!gate.close(Duration::from_millis(50)));
    // Closed: no new pass.
    assert!(gate.enter().is_none());
    drop(pass);
    assert!(gate.close(Duration::from_millis(50)));
}

unsafe extern "C" fn slow(_: *mut c_void, _: *const *const f32, _: u32, _: u32) {
    std::thread::sleep(Duration::from_millis(20));
}

/// strih-snv, v0.3.0 (camera-box#1485): a block held 6.7 ms past its 0.33 ms, partly inside OBS's
/// source mutexes. However long OBS holds the callback, the pump's block only queues.
#[test]
fn obs_holding_the_callback_never_lengthens_the_pump_s_block() {
    let feed = Arc::new(Feed::default());
    let (p, c) = ring(2, 4800);
    let mut sink = Sink::new(p, 2, Arc::clone(&feed));
    let mut feeder = Feeder::new(
        slow,
        std::ptr::null_mut(),
        c,
        2,
        Arc::clone(&feed),
        Gate::new(),
    );
    let input = [0.25; 64];
    let mut worst = Duration::ZERO;
    for _ in 0..3 {
        let t = Instant::now();
        push(&mut sink, 32, &input);
        worst = worst.max(t.elapsed());
        assert_eq!(feeder.drain(), Some(32));
    }
    assert!(
        worst < Duration::from_millis(2),
        "a pump block took {worst:?}"
    );
    let obs_ns = feed.max_obs_ns.load(Ordering::Relaxed);
    assert!(obs_ns >= 20_000_000, "max_obs_ns {obs_ns}");
    assert_eq!(feed.delivered.load(Ordering::Relaxed), 96);
}

/// camera-box#1485: a start skip is timed to the second for the first 30 s, then the 10 s contract.
#[test]
fn the_status_line_comes_every_second_for_the_first_30_then_every_10_s() {
    let waits: Vec<Duration> = (0..32).map(status_wait).collect();
    assert!(
        waits[..30].iter().all(|&w| w == Duration::from_secs(1)),
        "{waits:?}"
    );
    assert_eq!(waits[30], Duration::from_secs(10));
    assert_eq!(waits[31], Duration::from_secs(10));
    assert_eq!(status_wait(u32::MAX), Duration::from_secs(10));
    let first_30: Duration = waits[..30].iter().sum();
    assert_eq!(first_30, Duration::from_secs(30));
}
