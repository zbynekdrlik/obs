use super::*;

use std::sync::Mutex as StdMutex;

/// What the C callback saw: (channels, frames, samples per channel).
type Seen = (u32, u32, Vec<Vec<f32>>);
static SEEN: StdMutex<Vec<Seen>> = StdMutex::new(Vec::new());

unsafe extern "C" fn record(
    _opaque: *mut c_void,
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
    SEEN.lock().unwrap().push((channels, frames, chans));
}

#[test]
fn the_sink_hands_each_block_as_float_planes_and_counts_the_frames() {
    let delivered = Arc::new(AtomicU64::new(0));
    let gate = Arc::new(Mutex::new(true));
    let mut sink = Sink::new(
        record,
        std::ptr::null_mut(),
        2,
        Arc::clone(&delivered),
        Arc::clone(&gate),
    );
    // Two channels of 4 frames, channel-major.
    let input = [0.5, -0.25, 1.0, 0.0, -1.0, 0.125, 0.75, -0.5];
    for _ in 0..2 {
        let mut out: [f64; 0] = [];
        let mut block = Block::new(4, &input, &mut out);
        sink.process(&mut block);
    }
    let seen = SEEN.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (channels, frames, chans) in seen.iter() {
        assert_eq!((*channels, *frames), (2, 4));
        assert_eq!(chans[0], [0.5, -0.25, 1.0, 0.0]);
        assert_eq!(chans[1], [-1.0, 0.125, 0.75, -0.5]);
    }
    assert_eq!(delivered.load(Ordering::Relaxed), 8);
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

static GATED: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn count(_: *mut c_void, _: *const *const f32, _: u32, _: u32) {
    GATED.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn a_closed_gate_keeps_every_later_block_from_the_plugin() {
    let delivered = Arc::new(AtomicU64::new(0));
    let gate = Arc::new(Mutex::new(true));
    let mut sink = Sink::new(
        count,
        std::ptr::null_mut(),
        1,
        Arc::clone(&delivered),
        Arc::clone(&gate),
    );
    let input = [0.0; 32];
    let mut out: [f64; 0] = [];
    sink.process(&mut Block::new(32, &input, &mut out));
    close_gate(&gate);
    sink.process(&mut Block::new(32, &input, &mut out));
    sink.process(&mut Block::new(32, &input, &mut out));
    assert_eq!(GATED.load(Ordering::Relaxed), 1);
    assert_eq!(delivered.load(Ordering::Relaxed), 32);
}

#[test]
fn the_sample_rate_is_dlinks() {
    assert_eq!(dante_input_sample_rate(), 48_000);
}
