use super::*;

use std::sync::Mutex as StdMutex;

/// What the C callback saw: (channels, frames, samples per channel, timestamp ns).
type Seen = (u32, u32, Vec<Vec<f32>>, u64);
/// One test's callbacks: the callback's `opaque` points here, so parallel tests do not mix.
type SeenBox = StdMutex<Vec<Seen>>;

unsafe extern "C" fn record(
    opaque: *mut c_void,
    planes: *const *const f32,
    channels: u32,
    frames: u32,
    timestamp_ns: u64,
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
        .push((channels, frames, chans, timestamp_ns));
}

/// The app rate the stamp tests run at.
const RATE: u32 = 48_000;

/// A sink and a feeder over a ring of `frames` frames of `channels` at [`RATE`].
fn pair(seen: &SeenBox, channels: usize, frames: usize) -> (Sink, Feeder, Arc<Feed>, Arc<Gate>) {
    pair_with(seen, channels, frames, STAMP_SLOTS)
}

/// [`pair`] with a stamp ring of `slots` entries.
fn pair_with(
    seen: &SeenBox,
    channels: usize,
    frames: usize,
    slots: usize,
) -> (Sink, Feeder, Arc<Feed>, Arc<Gate>) {
    let feed = Arc::new(Feed::default());
    let gate = Gate::new();
    let (p, c) = ring(channels, frames);
    let (sp, sc) = stamps(slots);
    let sink = Sink::new(p, sp, channels, Arc::clone(&feed));
    let feeder = Feeder::new(
        record,
        (seen as *const SeenBox).cast_mut().cast(),
        c,
        sc,
        channels,
        RATE,
        Arc::clone(&feed),
        Arc::clone(&gate),
    );
    (sink, feeder, feed, gate)
}

/// One block with its play time `ns`.
fn push_at(sink: &mut Sink, frames: usize, input: &[f64], ns: i64) {
    let mut out: [f64; 0] = [];
    let mut block = Block::new(frames, input, &mut out).played_at(ns);
    sink.process(&mut block);
}

/// ns of `frames` frames at [`RATE`].
fn frames_ns(frames: u64) -> u64 {
    frames * 1_000_000_000 / u64::from(RATE)
}

/// One block with its media start (app frames) and its play time `ns`.
fn push_media(sink: &mut Sink, frames: usize, input: &[f64], start: u64, ns: i64) {
    let mut out: [f64; 0] = [];
    let mut block = Block::new(frames, input, &mut out).at(start).played_at(ns);
    sink.process(&mut block);
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
    let (channels, frames, chans, _) = &seen[0];
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

/// obs#53 (camera-box issue 1527): each chunk handed to OBS carries the play time of its first
/// frame: the block's play time (dlink v0.6.132, dstack#295) plus the frames before it in the block.
#[test]
fn each_chunk_carries_the_play_time_of_its_first_frame() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, _feed, _gate) = pair(&seen_box, 1, 4800);
    let t0: i64 = 5_000_000_000;
    // Ten 128-frame blocks, each one 128 frames later on the app clock.
    for k in 0..10 {
        push_at(
            &mut sink,
            128,
            &[0.0; 128],
            t0 + (frames_ns(128 * k) as i64),
        );
    }
    assert_eq!(feeder.drain(), Some(1280));
    let seen = seen_box.lock().unwrap().clone();
    let sizes: Vec<u32> = seen.iter().map(|s| s.1).collect();
    assert_eq!(sizes, [480, 480, 320]);
    // Chunk 1 starts at frame 0 (block 0), chunk 2 at frame 480 (block 3, frame 96 in it),
    // chunk 3 at frame 960 (block 7, frame 64 in it): the block's play time plus the frames
    // before it in the block, each rounded down to the ns as dlink's and ours are.
    let want: Vec<u64> = [(0u64, 0u64), (3, 96), (7, 64)]
        .iter()
        .map(|&(b, f)| t0 as u64 + frames_ns(128 * b) + frames_ns(f))
        .collect();
    let got: Vec<u64> = seen.iter().map(|s| s.3).collect();
    assert_eq!(got, want);
}

/// obs#53: the Dante media clock and the system clock ran ~5.9 ppm apart on stream-snv
/// (21 ms an hour). The old stamp (first arrival + the sample count) drifted and was re-anchored
/// by ~21 ms at once: the A/V stepped. The play times follow the drift block by block, so the
/// stamps never step: each chunk's stamp is its block's play time, never a count from the start.
#[test]
fn the_stamps_follow_a_clock_drift_block_by_block_with_no_step() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 48_000);
    let t0: i64 = 1_000_000_000;
    // 6000 ppm (a thousandfold the real drift, to see it in 50 blocks): each block's play time is
    // 128 frames * (1 + 6000e-6) after the last, 16 us more than the sample count a block.
    let step = frames_ns(128) as f64 * (1.0 + 6000e-6);
    let mut pushed = Vec::new();
    for k in 0..50 {
        let ns = t0 + (step * k as f64).round() as i64;
        push_at(&mut sink, 128, &[0.0; 128], ns);
        pushed.push(ns as u64);
        // One block a drain: each chunk is one block.
        assert_eq!(feeder.drain(), Some(128));
    }
    let seen = seen_box.lock().unwrap().clone();
    let got: Vec<u64> = seen.iter().map(|s| s.3).collect();
    assert_eq!(
        got, pushed,
        "every chunk is its block's play time, never a count from the start"
    );
    // 49 blocks counted from the first would be 130.6 ms; the play times are 784 us later.
    let counted = pushed[0] + frames_ns(128 * 49);
    assert!(
        pushed[49] > counted + 700_000,
        "the stamps follow the drift: {} vs {counted}",
        pushed[49]
    );
    assert_eq!(
        feed.stamp_jumps.load(Ordering::Relaxed),
        0,
        "a drift is no jump"
    );
}

/// obs#53: a block with no play time yet (no media clock) hands 0: the plugin stamps it with the
/// system clock then.
#[test]
fn a_block_with_no_play_time_hands_0() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, _feed, _gate) = pair(&seen_box, 1, 64);
    push(&mut sink, 4, &[0.1, 0.2, 0.3, 0.4]);
    assert_eq!(feeder.drain(), Some(4));
    assert_eq!(seen_box.lock().unwrap()[0].3, 0);
}

/// obs#53: a play time more than 1 ms off its continuation (a gap, a media re-anchor) is a jump:
/// counted, and the stamp follows it (no re-anchor of our own).
#[test]
fn a_play_time_off_its_continuation_by_more_than_1_ms_counts_a_jump() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 4800);
    let t0: i64 = 2_000_000_000;
    push_at(&mut sink, 128, &[0.0; 128], t0);
    assert_eq!(feeder.drain(), Some(128));
    // 0.5 ms off: no jump.
    let next = t0 + frames_ns(128) as i64 + 500_000;
    push_at(&mut sink, 128, &[0.0; 128], next);
    assert_eq!(feeder.drain(), Some(128));
    assert_eq!(feed.stamp_jumps.load(Ordering::Relaxed), 0);
    // 21 ms off: a jump, and the stamp is the play time.
    let jumped = next + frames_ns(128) as i64 + 21_000_000;
    push_at(&mut sink, 128, &[0.0; 128], jumped);
    assert_eq!(feeder.drain(), Some(128));
    assert_eq!(feed.stamp_jumps.load(Ordering::Relaxed), 1);
    assert_eq!(seen_box.lock().unwrap()[2].3, jumped as u64);
}

/// obs#53 review: a block the full sample ring drops takes its play time with it; the next block
/// that lands is stamped with its own play time, not one counted on from the dropped block.
#[test]
fn a_dropped_block_s_play_time_never_stamps_the_next_block() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 6);
    let t0: i64 = 3_000_000_000;
    push_at(&mut sink, 4, &[0.0; 4], t0);
    // 4 of 6 frames used: this one is dropped.
    push_at(&mut sink, 4, &[0.0; 4], t0 + 7_000_000);
    assert_eq!(feed.dropped.load(Ordering::Relaxed), 4);
    assert_eq!(feeder.drain(), Some(4));
    let t2 = t0 + 9_000_000;
    push_at(&mut sink, 4, &[0.0; 4], t2);
    assert_eq!(feeder.drain(), Some(4));
    let got: Vec<u64> = seen_box.lock().unwrap().iter().map(|s| s.3).collect();
    assert_eq!(got, [t0 as u64, t2 as u64]);
}

/// obs#53 review: a full stamp ring counts the lost play time; the chunk runs on by the sample
/// count from the last one.
#[test]
fn a_full_stamp_ring_counts_the_lost_play_time() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair_with(&seen_box, 1, 64, 1);
    let t0: i64 = 4_000_000_000;
    push_at(&mut sink, 4, &[0.0; 4], t0);
    push_at(&mut sink, 4, &[0.0; 4], t0 + 1_000_000);
    assert_eq!(feed.stamps_dropped.load(Ordering::Relaxed), 1);
    assert_eq!(feeder.drain(), Some(8));
    assert_eq!(seen_box.lock().unwrap()[0].3, t0 as u64);
}

/// obs#53 review: a negative play time is no time on OBS's clock: counted, handed as 0 (the
/// plugin stamps it with the system clock).
#[test]
fn a_negative_play_time_is_counted_and_handed_as_0() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 64);
    push_at(&mut sink, 4, &[0.0; 4], -7);
    assert_eq!(feeder.drain(), Some(4));
    assert_eq!(seen_box.lock().unwrap()[0].3, 0);
    assert_eq!(feed.bad_stamps.load(Ordering::Relaxed), 1);
}

/// obs#53 review: chunks before the media clock carry 0 (the system clock); the first play time
/// after them is taken as it is, not checked against a system-clock continuation.
#[test]
fn the_first_play_time_after_system_clock_chunks_is_no_jump() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 64);
    push(&mut sink, 4, &[0.0; 4]);
    assert_eq!(feeder.drain(), Some(4));
    push_at(&mut sink, 4, &[0.0; 4], 6_000_000_000);
    assert_eq!(feeder.drain(), Some(4));
    let got: Vec<u64> = seen_box.lock().unwrap().iter().map(|s| s.3).collect();
    assert_eq!(got, [0, 6_000_000_000]);
    assert_eq!(feed.stamp_jumps.load(Ordering::Relaxed), 0);
}

/// obs#53 (stream-snv 14.11.2026 17:11:03): the pump woke late and skipped 8 blocks (512 app frames,
/// 10.667 ms); the next block's play time was correctly 10.667 ms later, but it was handed right
/// after the last one, so OBS's timeline lost 10.667 ms and the stamp-placed ASRC slewed the audio
/// for ~11 s. A skip in the blocks' media time is filled with exactly that much silence: OBS's
/// timeline stays contiguous, the stamps never jump.
#[test]
fn a_skip_in_media_time_is_filled_with_silence_and_the_stamps_stay_contiguous() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 4800);
    let t0: i64 = 7_000_000_000;
    push_media(&mut sink, 128, &[1.0; 128], 9_600, t0);
    // 512 frames skipped: the next block starts at 9_600 + 128 + 512.
    push_media(
        &mut sink,
        128,
        &[1.0; 128],
        10_240,
        t0 + frames_ns(640) as i64,
    );
    assert_eq!(feeder.drain(), Some(768));
    let seen = seen_box.lock().unwrap().clone();
    let samples: Vec<f32> = seen.iter().flat_map(|s| s.2[0].clone()).collect();
    let mut want = vec![1.0f32; 128];
    want.extend(vec![0.0f32; 512]);
    want.extend(vec![1.0f32; 128]);
    assert_eq!(
        samples, want,
        "the skip is silence, the blocks around it whole"
    );
    let stamps: Vec<u64> = seen.iter().map(|s| s.3).collect();
    // The second chunk starts at frame 480 (MAX_CHUNK), inside the silence: block 1's play time
    // carried on by 480 frames, the timeline contiguous through the skip.
    assert_eq!(stamps, [t0 as u64, t0 as u64 + frames_ns(480)]);
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), 512);
    assert_eq!(feed.gaps.load(Ordering::Relaxed), 1);
    assert_eq!(
        feed.stamp_jumps.load(Ordering::Relaxed),
        0,
        "a filled skip is no jump"
    );
}

/// obs#53 review: two channels: the silence fills every channel of the skipped frames.
#[test]
fn a_skip_is_filled_on_every_channel() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 2, 4800);
    push_media(&mut sink, 4, &[1.0; 8], 0, 1_000_000_000);
    push_media(
        &mut sink,
        4,
        &[1.0; 8],
        4 + 3,
        1_000_000_000 + frames_ns(7) as i64,
    );
    assert_eq!(feeder.drain(), Some(11));
    let seen = seen_box.lock().unwrap().clone();
    let want = [1.0f32, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    assert_eq!(seen[0].2[0], want);
    assert_eq!(seen[0].2[1], want);
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), 3);
}

/// obs#53 review: a skip of exactly GAP_FILL_MAX_FRAMES is filled; one frame more is not.
#[test]
fn a_skip_of_exactly_the_bound_is_filled() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 48_000);
    push_media(&mut sink, 4, &[1.0; 4], 0, 1_000_000_000);
    push_media(
        &mut sink,
        4,
        &[1.0; 4],
        4 + GAP_FILL_MAX_FRAMES,
        2_000_000_000,
    );
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), GAP_FILL_MAX_FRAMES);
    assert!(feeder.drain().is_some());
}

/// obs#53 review: a skip the full ring has no room for is counted apart (not filled, not dropped
/// audio); the block after it still lands.
#[test]
fn a_skip_the_full_ring_cannot_take_is_counted_unfilled() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 12);
    push_media(&mut sink, 4, &[1.0; 4], 0, 1_000_000_000);
    // 10 frames skipped: 8 frames of room left, the silence does not fit; the block does.
    push_media(
        &mut sink,
        4,
        &[1.0; 4],
        14,
        1_000_000_000 + frames_ns(14) as i64,
    );
    assert_eq!(feed.gaps_unfilled.load(Ordering::Relaxed), 1);
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), 0);
    assert_eq!(feed.dropped.load(Ordering::Relaxed), 0);
    assert_eq!(feeder.drain(), Some(8));
}

/// obs#53 review: a block the full ring drops is filled with silence by the next block once the
/// ring has room, so OBS's timeline keeps its frames.
#[test]
fn a_block_the_full_ring_dropped_is_filled_by_the_next_block() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 6);
    push_media(&mut sink, 4, &[1.0; 4], 0, 1_000_000_000);
    // 4 of 6 frames used: this block is dropped.
    push_media(
        &mut sink,
        4,
        &[1.0; 4],
        4,
        1_000_000_000 + frames_ns(4) as i64,
    );
    assert_eq!(feed.dropped.load(Ordering::Relaxed), 4);
    assert_eq!(feeder.drain(), Some(4));
    // The next block: the dropped block's 4 frames are filled first.
    push_media(
        &mut sink,
        2,
        &[1.0; 2],
        8,
        1_000_000_000 + frames_ns(8) as i64,
    );
    assert_eq!(feeder.drain(), Some(6));
    let samples: Vec<f32> = seen_box
        .lock()
        .unwrap()
        .iter()
        .flat_map(|s| s.2[0].clone())
        .collect();
    assert_eq!(samples, [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0]);
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), 4);
    assert_eq!(feed.stamp_jumps.load(Ordering::Relaxed), 0);
}

/// obs#56 (dstack#303 bisect): with `rx_stamps = "count"` the chunks are stamped by the sample
/// count from the first play time; later play times (drifting here at 6000 ppm) are not followed.
#[test]
fn counting_stamps_by_the_sample_count_from_the_first_play_time() {
    let seen_box = SeenBox::default();
    let (mut sink, feeder, _feed, _gate) = pair(&seen_box, 1, 48_000);
    let mut feeder = feeder.counting();
    let t0: i64 = 1_000_000_000;
    let step = frames_ns(128) as f64 * (1.0 + 6000e-6);
    for k in 0..10 {
        push_at(
            &mut sink,
            128,
            &[0.0; 128],
            t0 + (step * k as f64).round() as i64,
        );
        assert_eq!(feeder.drain(), Some(128));
    }
    let got: Vec<u64> = seen_box.lock().unwrap().iter().map(|s| s.3).collect();
    let want: Vec<u64> = (0..10).map(|k| t0 as u64 + frames_ns(128 * k)).collect();
    assert_eq!(got, want);
}

/// obs#56 review: counting keeps the diagnostics: before the first play time it hands 0, a bad
/// first play time is counted, and a skip is still filled with silence.
#[test]
fn counting_keeps_the_zero_start_the_bad_stamp_count_and_the_fill() {
    let seen_box = SeenBox::default();
    let (mut sink, feeder, feed, _gate) = pair(&seen_box, 1, 4800);
    let mut feeder = feeder.counting();
    push(&mut sink, 4, &[0.0; 4]);
    assert_eq!(feeder.drain(), Some(4));
    push_media(&mut sink, 4, &[1.0; 4], 0, -7);
    assert_eq!(feeder.drain(), Some(4));
    assert_eq!(feed.bad_stamps.load(Ordering::Relaxed), 1);
    push_media(&mut sink, 4, &[1.0; 4], 10, 1_000_000_000);
    assert_eq!(feeder.drain(), Some(10));
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), 6);
    push_media(
        &mut sink,
        4,
        &[1.0; 4],
        14,
        1_000_000_000 + frames_ns(4) as i64,
    );
    assert_eq!(feeder.drain(), Some(4));
    let got: Vec<u64> = seen_box.lock().unwrap().iter().map(|s| s.3).collect();
    // No play time (0), a bad one (0), the gap chunk still on the bad one (0); then the good play
    // time (frame 14) is the count's new origin: the chunk at frame 18 counts 4 frames from it.
    assert_eq!(got, [0, 0, 0, 1_000_000_000 + frames_ns(4)]);
    assert!(feed.bad_stamps.load(Ordering::Relaxed) >= 1);
}

/// obs#53: a backward media start (a schedule started again) has nothing to fill.
#[test]
fn a_backward_media_start_is_not_filled() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 4800);
    push_media(&mut sink, 128, &[1.0; 128], 1_000, 1_000_000_000);
    push_media(&mut sink, 128, &[1.0; 128], 500, 1_010_000_000);
    assert_eq!(feeder.drain(), Some(256));
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), 0);
}

/// obs#53: a forward jump past [`GAP_FILL_MAX_FRAMES`] is a schedule started again, not a skip:
/// no second of silence is written.
#[test]
fn a_media_jump_past_the_fill_bound_is_not_filled() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 4800);
    push_media(&mut sink, 128, &[1.0; 128], 0, 1_000_000_000);
    push_media(
        &mut sink,
        128,
        &[1.0; 128],
        128 + GAP_FILL_MAX_FRAMES + 1,
        5_000_000_000,
    );
    assert_eq!(feeder.drain(), Some(256));
    assert_eq!(feed.gap_frames.load(Ordering::Relaxed), 0);
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

unsafe extern "C" fn slow(_: *mut c_void, _: *const *const f32, _: u32, _: u32, _: u64) {
    std::thread::sleep(Duration::from_millis(20));
}

/// strih-snv, v0.3.0 (camera-box#1485): a block held 6.7 ms past its 0.33 ms, partly inside OBS's
/// source mutexes. However long OBS holds the callback, the pump's block only queues.
#[test]
fn obs_holding_the_callback_never_lengthens_the_pump_s_block() {
    let feed = Arc::new(Feed::default());
    let (p, c) = ring(2, 4800);
    let (sp, sc) = stamps(STAMP_SLOTS);
    let mut sink = Sink::new(p, sp, 2, Arc::clone(&feed));
    let mut feeder = Feeder::new(
        slow,
        std::ptr::null_mut(),
        c,
        sc,
        2,
        RATE,
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

/// obs#94 (camera-box#1527): each stamped chunk's lead at the hand-off to OBS = the end of its
/// audio (stamp + length) less the time now (a mix tick needs the chunk only by its end). The
/// window keeps the SMALLEST lead for the status line, which takes it. A lead <= 0 is information,
/// not a verdict: only the OBS mix's probe says a tick left the source out (late_to_mix).
#[test]
fn the_feeder_measures_each_chunk_s_lead_to_its_end_at_the_hand_off() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 4096);
    assert_eq!(feed.min_lead.peek_us(), None);
    // 960 frames at 48 kHz = 20 ms, its first sample 5 ms in the past: late to its start, its end
    // still ~15 ms ahead, in time for the mix.
    push_at(&mut sink, 960, &[0.0; 960], obs_now_ns() - 5_000_000);
    assert_eq!(feeder.drain(), Some(480 + 480));
    let lead = feed.min_lead.peek_us().expect("a stamped chunk has a lead");
    assert!((0..=15_000).contains(&lead), "lead {lead} us");
    // A peek leaves the window as it is.
    assert_eq!(feed.min_lead.peek_us(), Some(lead));
    assert_eq!(feed.min_lead.take_us(), Some(lead));
    // A late chunk first (128 frames = 2.667 ms, first sample 30 ms ago: its end ~27 ms ago),
    // then one 40 ms ahead: the window keeps the late one, not the last one.
    push_at(&mut sink, 128, &[0.0; 128], obs_now_ns() - 30_000_000);
    assert_eq!(feeder.drain(), Some(128));
    push_at(&mut sink, 128, &[0.0; 128], obs_now_ns() + 40_000_000);
    assert_eq!(feeder.drain(), Some(128));
    let lead = feed
        .min_lead
        .take_us()
        .expect("the window had stamped chunks");
    assert!((-30_000..=-27_333).contains(&lead), "lead {lead} us");
    // Taking it opens a new, empty window.
    assert_eq!(feed.min_lead.take_us(), None);
}

/// obs#94: a chunk with no play time goes to OBS with stamp 0 (plugin.c stamps it now): no lead.
#[test]
fn an_unstamped_chunk_has_no_lead() {
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, feed, _gate) = pair(&seen_box, 1, 1024);
    push(&mut sink, 128, &[0.0; 128]);
    assert_eq!(feeder.drain(), Some(128));
    assert_eq!(seen_box.lock().unwrap()[0].3, 0);
    assert_eq!(feed.min_lead.peek_us(), None);
}

/// obs#94: late_to_mix and mixed_late are the OBS mix's own counts, read through plugin.c's probe on
/// the status thread with the plugin's `opaque`; 0 before a probe is set and after it is cleared.
unsafe extern "C" fn mix_counts(opaque: *mut c_void, short_ticks: *mut u64, mixed_late: *mut u64) {
    // SAFETY: the test passes two live AtomicU64; the device passes two live out-pointers.
    let c = unsafe { &*(opaque as *const [AtomicU64; 2]) };
    unsafe {
        *short_ticks = c[0].load(Ordering::Relaxed);
        *mixed_late = c[1].load(Ordering::Relaxed);
    }
}

#[test]
fn late_to_mix_and_mixed_late_are_the_mix_probe_s_counts() {
    let feed = Feed::default();
    assert_eq!(feed.mix_probe.read(), MixCounts::default());
    let counts = [AtomicU64::new(7), AtomicU64::new(3)];
    feed.mix_probe.set(Some((
        mix_counts,
        (&counts as *const [AtomicU64; 2]).cast_mut().cast(),
    )));
    assert_eq!(
        feed.mix_probe.read(),
        MixCounts {
            short_ticks: 7,
            mixed_late: 3
        }
    );
    counts[0].store(12, Ordering::Relaxed);
    counts[1].store(20, Ordering::Relaxed);
    assert_eq!(
        feed.mix_probe.read(),
        MixCounts {
            short_ticks: 12,
            mixed_late: 20
        }
    );
    feed.mix_probe.set(None);
    assert_eq!(feed.mix_probe.read(), MixCounts::default());
}

/// obs#96 (camera-box#1425): with a stamp margin every stamped chunk reaches OBS stamped that much
/// after dlink's play time (the hand-off margin), so a chunk handed after the end of its own audio
/// no longer sits out the mix tick that needs it; its lead is measured to the shifted end. An
/// unstamped chunk (stamp 0: plugin.c stamps it now) stays 0.
#[test]
fn a_stamp_margin_stamps_every_chunk_that_much_after_its_play_time() {
    let seen_box = SeenBox::default();
    let (mut sink, feeder, feed, _gate) = pair(&seen_box, 1, 1024);
    let mut feeder = feeder.stamp_margin_ns(4_667_000);
    let t0: i64 = 1_000_000_000_000;
    push_at(&mut sink, 128, &[0.0; 128], t0);
    push_at(&mut sink, 128, &[0.0; 128], t0 + frames_ns(128) as i64);
    assert_eq!(feeder.drain(), Some(256));
    let seen = seen_box.lock().unwrap().clone();
    let stamps: Vec<u64> = seen.iter().map(|s| s.3).collect();
    assert_eq!(stamps[0], t0 as u64 + 4_667_000, "{stamps:?}");
    // the stamps stay continuous: the second chunk's stamp follows the first by its frames
    let total: u64 = seen.iter().map(|s| u64::from(s.1)).sum();
    assert_eq!(total, 256);
    assert_eq!(
        *stamps.last().unwrap(),
        t0 as u64 + 4_667_000 + frames_ns(256 - u64::from(seen.last().unwrap().1)),
        "{stamps:?}"
    );
    // the lead is to the shifted end: these stamps sit far in the past, so it is negative by about
    // the time since t0, less the margin
    assert!(feed.min_lead.peek_us().is_some());

    // without a margin the same chunk gets dlink's play time itself
    let seen_box = SeenBox::default();
    let (mut sink, mut feeder, _feed, _gate) = pair(&seen_box, 1, 1024);
    push_at(&mut sink, 128, &[0.0; 128], t0);
    assert_eq!(feeder.drain(), Some(128));
    assert_eq!(seen_box.lock().unwrap()[0].3, t0 as u64);
}

/// obs#96 (camera-box#1425): the pump wakes the feeder the moment it has written a block, so a chunk
/// no longer waits up to FEED_EVERY (2 ms) in the ring before OBS gets it (strih-snv: every status
/// window had chunks handed over 1+ ms after the end of their own audio). The test thread stands in
/// for the feeder: a written block unparks it at once; without the wake its park would last 10 s.
#[test]
fn the_pump_wakes_the_feeder_when_it_writes_a_block() {
    let feed = Arc::new(Feed::default());
    let (p, _c) = ring(1, 1024);
    let (sp, _sc) = stamps(STAMP_SLOTS);
    let mut sink = Sink::new(p, sp, 1, Arc::clone(&feed));
    feed.feeder_thread
        .set(std::thread::current())
        .expect("the feeder thread is set once");
    push_at(&mut sink, 64, &[0.0; 64], 1_000_000_000);
    let t = Instant::now();
    std::thread::park_timeout(Duration::from_secs(10));
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "the written block did not wake the feeder: parked {:?}",
        t.elapsed()
    );
}
