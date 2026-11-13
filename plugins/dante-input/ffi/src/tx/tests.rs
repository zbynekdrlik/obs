use super::*;
use dlink::bridge::{chunk_ns, player};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;

/// The stream PC's app buffer (dstack #247): OBS's 1024 frames at `obs_rate` and one 128-frame
/// pump block at the 96 kHz device rate.
fn buffer(obs_rate: u32) -> AppBuffer {
    AppBuffer::new(chunk_ns(1024, obs_rate), chunk_ns(128, 96_000))
}

/// One pump block of `outputs` TX channels filled by `out`; the channels, channel-major.
fn pump(out: &mut TxOut, outputs: usize) -> Vec<f64> {
    let input: [f64; 0] = [];
    let mut output = vec![0.0; BLOCK * outputs];
    let mut block = Block::new(BLOCK, &input, &mut output);
    out.fill(&mut block);
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
fn the_fifo_target_is_obs_s_chunk_and_one_pump_block() {
    // dstack #247 (v0.6.112): 21.333 ms of OBS's 1024 frames at 48 kHz + 1.333 ms of a 128-frame
    // block at 96 kHz = 22.667 ms, not the fixed 40 ms before.
    let (tx_in, _out) = tx(1, RATE, RATE, buffer(RATE)).unwrap();
    assert_eq!(buffer(RATE).target_ns(), 21_333_333 + 1_333_333);
    assert!(
        (tx_in.target_ms() - 22.666666).abs() < 1e-6,
        "{}",
        tx_in.target_ms()
    );
}
