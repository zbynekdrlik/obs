use super::*;
use dlink::bridge::{chunk_ns, player};
use dlink::{Block, Process};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;

/// The stream PC's app buffer (dstack #247): OBS's 1024 frames at `obs_rate` and one 128-frame
/// pump block at the 96 kHz device rate.
fn buffer(obs_rate: u32) -> AppBuffer {
    AppBuffer::new(chunk_ns(1024, obs_rate), chunk_ns(128, 96_000))
}

/// One TX block of `outputs` TX channels filled by `out` (the TX thread's pull); the channels, channel-major.
fn pump(out: &mut TxPull, outputs: usize) -> Vec<f64> {
    let input: [f64; 0] = [];
    let mut output = vec![0.0; BLOCK * outputs];
    let mut block = Block::new(BLOCK, &input, &mut output);
    out.process(&mut block);
    output
}

/// One pump block starting at `start` (the block's media start, output frames).
fn pump_at(out: &mut TxPull, outputs: usize, start: u64) -> Vec<f64> {
    let input: [f64; 0] = [];
    let mut output = vec![0.0; BLOCK * outputs];
    let mut block = Block::new(BLOCK, &input, &mut output).at(start);
    out.process(&mut block);
    output
}

/// OBS's 1024-frame mix: a ramp on the left, its negation on the right.
fn obs_chunk(from: usize) -> (Vec<f32>, Vec<f32>) {
    let left: Vec<f32> = (0..1024)
        .map(|f| ((from + f) % 1000) as f32 / 2000.0 + 0.1)
        .collect();
    let right = left.iter().map(|x| -x).collect();
    (left, right)
}

#[test]
fn obs_s_mix_reaches_the_tx_outputs_after_the_fifo_primes_left_and_right_kept_apart() {
    let (tx_in, mut tx_out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    assert_eq!(tx_in.track(), 1);
    assert!(
        pump(&mut tx_out, 2).iter().all(|&x| x == 0.0),
        "silence while the FIFO is empty"
    );
    assert!(tx_in.counts().priming);

    // OBS hands 1024 frames every 1024 frames' time; the pump takes 128 a block.
    let mut sent = 0;
    let mut heard = Vec::new();
    for _ in 0..40 {
        let (l, r) = obs_chunk(sent);
        tx_in.push(&[&l, &r], 1024);
        sent += 1024;
        for _ in 0..8 {
            heard.push(pump(&mut tx_out, 2));
        }
    }
    let c = tx_in.counts();
    assert!(!c.priming, "{c:?}");
    // The silent block before OBS sent anything counts as played too.
    assert_eq!(c.played, (1 + 40 * 8) * BLOCK as u64);
    assert_eq!(c.overflows, 0);
    let last = heard.last().unwrap();
    let (left, right) = last.split_at(BLOCK);
    assert!(
        left.iter().all(|&x| x > 0.05),
        "the left carries the mix: {left:?}"
    );
    for (l, r) in left.iter().zip(right) {
        assert!(
            (l + r).abs() < 1e-3,
            "the right is the left negated: {l} {r}"
        );
    }
}

#[test]
fn a_mono_mix_goes_out_on_both_channels() {
    let (tx_in, mut tx_out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    let mut last = Vec::new();
    for i in 0..40 {
        let (l, _) = obs_chunk(i * 1024);
        tx_in.push(&[&l], 1024);
        for _ in 0..8 {
            last = pump(&mut tx_out, 2);
        }
    }
    let (left, right) = last.split_at(BLOCK);
    assert!(left.iter().all(|&x| x > 0.05));
    assert_eq!(left, right);
}

#[test]
fn obs_stopping_counts_underruns_and_silence_goes_out() {
    let (tx_in, mut tx_out) = tx(2, RATE, RATE, buffer(RATE)).unwrap();
    for i in 0..20 {
        let (l, r) = obs_chunk(i * 1024);
        tx_in.push(&[&l, &r], 1024);
        for _ in 0..8 {
            pump(&mut tx_out, 2);
        }
    }
    assert_eq!(tx_in.counts().underruns, 0);
    let mut last = Vec::new();
    for _ in 0..100 {
        last = pump(&mut tx_out, 2);
    }
    let c = tx_in.counts();
    assert!(c.underruns > 0, "{c:?}");
    assert_eq!(c.track, 2);
    assert!(last.iter().all(|&x| x == 0.0));
}

#[test]
fn a_pump_that_does_not_read_counts_obs_s_frames_as_overflows() {
    let (tx_in, _tx_out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    let (l, r) = obs_chunk(0);
    // The FIFO holds player::FIFO_S (0.5 s): 24000 frames at 48 kHz, so 30 chunks overflow it.
    for _ in 0..30 {
        tx_in.push(&[&l, &r], 1024);
    }
    let held = (player::FIFO_S * f64::from(RATE)) as u64;
    assert!(
        tx_in.counts().overflows >= 30 * 1024 - held - 1024,
        "{:?}",
        tx_in.counts()
    );
    assert!(tx_in.counts().overflows > 0);
}

#[test]
fn a_device_with_more_outputs_gets_the_mix_on_the_first_two_only() {
    let (tx_in, mut tx_out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    let mut last = Vec::new();
    for i in 0..40 {
        let (l, r) = obs_chunk(i * 1024);
        tx_in.push(&[&l, &r], 1024);
        for _ in 0..8 {
            last = pump(&mut tx_out, 3);
        }
    }
    assert!(last[..BLOCK].iter().all(|&x| x > 0.05));
    assert!(last[2 * BLOCK..].iter().all(|&x| x == 0.0));
}

#[test]
fn obs_at_another_rate_than_the_device_s_app_rate_converts() {
    let (tx_in, mut tx_out) = tx(1, 44_100, RATE, buffer(44_100)).unwrap();
    assert_eq!(tx_in.rate(), 44_100);
    let mut last = Vec::new();
    for i in 0..60 {
        let (l, r) = obs_chunk(i * 1024);
        tx_in.push(&[&l, &r], 1024);
        // 1024 frames at 44.1 kHz are ~1114.6 at 48 kHz: 9 blocks of 128 roughly.
        let blocks = if i % 2 == 0 { 9 } else { 8 };
        for _ in 0..blocks {
            last = pump(&mut tx_out, 2);
        }
    }
    let c = tx_in.counts();
    assert!(!c.priming, "{c:?}");
    assert!(last[..BLOCK].iter().any(|&x| x > 0.05), "{c:?}");
}

#[test]
fn the_tx_holds_the_fifo_at_the_app_buffer_it_is_given() {
    // dstack #247: the target is the device's app buffer, not a constant of ours. A set app
    // buffer (30 ms) wins over the chunk and the block.
    let set = buffer(RATE).set(Some(30_000_000));
    let (tx_in, _out) = tx(1, RATE, RATE, set).unwrap();
    assert_eq!(tx_in.target_ms(), 30.0);
    let (tx_in, _out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    assert!(
        (tx_in.target_ms() - 22.666666).abs() < 1e-6,
        "{}",
        tx_in.target_ms()
    );
}

/// obs#56 (dstack#303, dlink v0.6.136): the TX passes each block's start to the player, so a block
/// that does not start where the last one ended (a pump rebootstrap skipping blocks) resyncs it
/// once instead of leaving the FIFO off its target; contiguous blocks never resync. The count is
/// the status line's tx_resyncs.
#[test]
fn a_block_off_the_last_one_s_end_resyncs_the_player_once() {
    let (tx_in, mut tx_out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    let mut start = 0u64;
    let mut sent = 0;
    for _ in 0..40 {
        let (l, r) = obs_chunk(sent);
        tx_in.push(&[&l, &r], 1024);
        sent += 1024;
        for _ in 0..8 {
            pump_at(&mut tx_out, 2, start);
            start += BLOCK as u64;
        }
    }
    assert!(!tx_in.counts().priming);
    assert_eq!(tx_in.counts().resyncs, 0, "contiguous blocks never resync");
    // The pump skipped 32 blocks (a rebootstrap): the next block starts 4096 frames on.
    start += 32 * BLOCK as u64;
    for _ in 0..4 {
        let (l, r) = obs_chunk(sent);
        tx_in.push(&[&l, &r], 1024);
        sent += 1024;
        for _ in 0..8 {
            pump_at(&mut tx_out, 2, start);
            start += BLOCK as u64;
        }
    }
    assert_eq!(tx_in.counts().resyncs, 1, "{:?}", tx_in.counts());
}

/// obs#56 review: a block wider than CHUNK is filled in pieces, and each piece passes its own
/// start (the block's start + the frames before it): contiguous 2048-frame blocks never resync,
/// so a piece passing the block's start again would read as a backward start and count one.
#[test]
fn a_block_filled_in_pieces_passes_each_piece_s_start() {
    let (tx_in, mut tx_out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    let input: [f64; 0] = [];
    let mut start = 0u64;
    let mut sent = 0;
    for _ in 0..20 {
        for _ in 0..2 {
            let (l, r) = obs_chunk(sent);
            tx_in.push(&[&l, &r], 1024);
            sent += 1024;
        }
        let mut output = vec![0.0; 2048 * 2];
        let mut block = Block::new(2048, &input, &mut output).at(start);
        tx_out.process(&mut block);
        start += 2048;
    }
    assert_eq!(tx_in.counts().resyncs, 0, "{:?}", tx_in.counts());
}

/// obs#56: OBS pushing a 1024-frame chunk every 21.3 ms is on time; a push over 1.5 chunk lengths
/// after the last is late, and the largest gap is the status line's window (a take resets it).
#[test]
fn a_push_over_one_and_a_half_chunks_late_counts_and_the_gap_max_is_per_window() {
    let c = Cadence::default();
    let chunk_ns = 1024 * 1_000_000_000 / u64::from(RATE); // 21_333_333
    let mut t = 1_000;
    c.note(t, 1024, RATE);
    for _ in 0..10 {
        t += chunk_ns + 2_000_000; // jitter, still on time
        c.note(t, 1024, RATE);
    }
    assert_eq!(c.late(), 0);
    assert_eq!(c.gap_max_us(true, t), 23_333);
    assert_eq!(c.gap_max_us(false, t), 0, "a take starts the next window");

    t += chunk_ns * 3 / 2 + 1_000; // just past the bound
    c.note(t, 1024, RATE);
    t += chunk_ns;
    c.note(t, 1024, RATE);
    assert_eq!(c.late(), 1);
    assert_eq!(c.gap_max_us(false, t), 32_000, "a peek leaves the window");
    assert_eq!(c.gap_max_us(true, t), 32_000);

    t += chunk_ns * 3 / 2 - 1_000; // just under the bound
    c.note(t, 1024, RATE);
    assert_eq!(c.late(), 1);
}

/// A writer stalled right now shows in the gap before its next push; the push that ends the
/// stall counts it late once.
#[test]
fn a_stall_still_open_shows_in_the_gap_and_counts_late_once_it_ends() {
    let c = Cadence::default();
    assert_eq!(c.gap_max_us(false, 1_000_000_000), 0, "no push yet");
    c.note(1_000, 1024, RATE);
    c.note(1_000 + 21_000_000, 1024, RATE);
    let stalled = 1_000 + 21_000_000 + 80_000_000;
    assert_eq!(c.gap_max_us(true, stalled), 80_000);
    assert_eq!(c.late(), 0, "no push ended it yet");
    c.note(stalled + 10_000_000, 1024, RATE);
    assert_eq!(c.late(), 1);
    assert_eq!(c.gap_max_us(true, stalled + 10_000_000), 90_000);
}

/// The first push has no gap; a shorter chunk sets a shorter bound for the next push.
#[test]
fn the_bound_follows_the_last_push_s_length() {
    let c = Cadence::default();
    c.note(5, 480, RATE); // 10 ms
    assert_eq!((c.late(), c.gap_max_us(false, 5)), (0, 0));
    c.note(5 + 16_000_000, 1024, RATE); // 16 ms after a 10 ms push: late
    assert_eq!(c.late(), 1);
}

/// TxIn::push feeds the cadence: two pushes give a gap, the status read takes it.
#[test]
fn obs_s_pushes_feed_the_cadence() {
    let (tx_in, _out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    let (l, r) = obs_chunk(0);
    tx_in.push(&[&l, &r], l.len());
    std::thread::sleep(std::time::Duration::from_millis(5));
    tx_in.push(&[&l, &r], l.len());
    let (late, gap) = tx_in.cadence(true);
    assert_eq!(late, 0);
    assert!(gap >= 5_000, "gap {gap} µs");
    // The window restarted: only the gap still open since the last push, far under the 5 ms.
    let (_, open) = tx_in.cadence(true);
    assert!(open < gap, "open {open} µs after a {gap} µs window");
}

/// obs#56 (dstack#303, dlink v0.6.140): on stream-snv OBS's TX write lands late inside its audio
/// pass (behind a slow output, ~10 ms) now and then, and every one turned into a ~22 ms dropout
/// (tx_underruns = tx_recentres, ~0.8/s). The FIFO still holds audio then, so a push 10 ms late
/// must not underrun or re-centre the player; pushes on time before and after.
#[test]
fn a_push_ten_ms_late_does_not_underrun_or_recentre() {
    // The FIFO stamps each write on the simulated time (frames at 48 kHz, in ns): since dlink
    // v0.6.143 the player steers on those stamps, so a wall clock would see 400 chunks in a
    // fraction of a second.
    let now = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let fifo = fifo_for(TX_CHANNELS, RATE).with_clock(Arc::new(move || {
        clock.load(std::sync::atomic::Ordering::Relaxed)
    }));
    let (tx_in, mut tx_out) = tx_on(fifo, 1, RATE, RATE, buffer(RATE)).unwrap();
    let at = |frames: u64| (frames * 1_000_000_000 / RATE as u64) as i64;
    const LATE: u64 = 480; // 10 ms at 48 kHz
    let push_at = |k: u64| k * 1024 + if k == 200 { LATE } else { 0 };
    let mut next = 0u64; // the next chunk to push
    let mut start = 0u64;
    let mut settled = None;
    for b in 0..(400 * 1024 / BLOCK as u64) {
        while push_at(next) <= b * BLOCK as u64 {
            now.store(at(push_at(next)), std::sync::atomic::Ordering::Relaxed);
            let (l, r) = obs_chunk((next * 1024) as usize);
            tx_in.push(&[&l, &r], 1024);
            next += 1;
        }
        if next == 150 && settled.is_none() {
            settled = Some(tx_in.counts());
        }
        now.store(at(b * BLOCK as u64), std::sync::atomic::Ordering::Relaxed);
        pump_at(&mut tx_out, 2, start);
        start += BLOCK as u64;
    }
    let before = settled.unwrap();
    let after = tx_in.counts();
    assert!(!before.priming, "settled before the late push: {before:?}");
    assert_eq!(
        (after.underruns, after.recentres),
        (before.underruns, before.recentres),
        "a push 10 ms late: {before:?} then {after:?}"
    );
}
