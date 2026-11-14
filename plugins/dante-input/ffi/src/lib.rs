//! The C ABI under the OBS `dante_input` source (obs#1, camera-box#1485): one dlink device (dstack)
//! per OBS process: its RX into OBS sources, and since v0.4.27 (obs#41) optionally one OBS track out
//! on two TX channels ([`tx`]). The pump's [`Sink`] only copies each block into a lock-free ring; the
//! `dante-input-obs` thread ([`Feeder`]) hands it to the C callback as float planes. OBS takes its
//! source mutexes in that call, so it never runs inside the pump's block (1.33 ms at 96 kHz since dlink v0.6.20) (v0.3.0 on
//! strih-snv: blocks of up to 6.7 ms, past the 5 ms margin, skipped).
//! The device's own log (dlink's clock choice, subscriptions, errors) and the status line (1 s for 30 s, then 10 s) go to
//! the C log callback, so they land in the OBS log.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dlink::{Block, Link, Map, Place, Process};

pub mod config;
pub mod status;
pub mod tray;
pub mod tx;

use config::InputConfig;
use status::Status;
use tx::{TxIn, TxOut};

/// OBS's log levels (util/base.h).
pub const LOG_ERROR: c_int = 100;
pub const LOG_WARNING: c_int = 200;
pub const LOG_INFO: c_int = 300;

/// The longest [`DanteInput::stop`] waits for a callback still inside the plugin.
pub const GATE_WAIT: Duration = Duration::from_secs(2);

/// How often the status line is logged (obs#1 log contract).
pub const STATUS_EVERY: Duration = Duration::from_secs(10);

/// The status line's cadence at start (camera-box#1485): every [`STATUS_START_EVERY`] for the
/// first [`STATUS_START_LINES`] lines, so a start skip is timed against the first packets and
/// camera-box's thread placement; then every [`STATUS_EVERY`].
pub const STATUS_START_EVERY: Duration = Duration::from_secs(1);
pub const STATUS_START_LINES: u32 = 30;

/// The wait before status line `n` (from 0).
pub fn status_wait(n: u32) -> Duration {
    if n < STATUS_START_LINES {
        STATUS_START_EVERY
    } else {
        STATUS_EVERY
    }
}

/// How often the feeder hands the ring's frames to OBS.
pub const FEED_EVERY: Duration = Duration::from_millis(2);

/// The ring between the pump and the feeder holds this much audio; a feeder that far behind loses
/// frames (counted as `obs_dropped`).
pub const RING_SECONDS: usize = 1;

/// The most frames one callback hands OBS (10 ms at 48 kHz).
pub const MAX_CHUNK: usize = 480;

/// `planes[c]` holds `frames` samples of channel `c`, -1.0 to 1.0. `timestamp_ns` is the play time
/// of the first frame on OBS's clock (`os_gettime_ns`): its block's play time from dlink (dstack#295)
/// plus the frames before it in the block; 0 while the device has no media clock (obs#53).
pub type AudioCb = unsafe extern "C" fn(
    opaque: *mut c_void,
    planes: *const *const f32,
    channels: u32,
    frames: u32,
    timestamp_ns: u64,
);

/// obs#53: a play time this far off its continuation is a jump (counted and logged, then followed).
pub const STAMP_JUMP_NS: u64 = 1_000_000;

/// Play-time entries the pump can queue ahead of the feeder (one per block).
pub const STAMP_SLOTS: usize = 8192;
pub type LogCb = unsafe extern "C" fn(level: c_int, msg: *const c_char);

static LOG: OnceLock<LogCb> = OnceLock::new();

/// Logs `msg` through the C callback (stderr before one is set).
pub fn log(level: c_int, msg: &str) {
    let line = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    match LOG.get() {
        // SAFETY: the callback is the plugin's, set once and valid for the process.
        Some(cb) => unsafe { cb(level, line.as_ptr()) },
        None => eprintln!("{msg}"),
    }
}

/// tracing's lines (dlink and dstack log with tracing) into the C log, one line per event.
struct LineWriter {
    level: c_int,
    buf: Vec<u8>,
}

impl Write for LineWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.buf);
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            log(self.level, &format!("dante-input dlink: {line}"));
        }
    }
}

struct MakeLineWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MakeLineWriter {
    type Writer = LineWriter;
    fn make_writer(&'a self) -> LineWriter {
        LineWriter {
            level: LOG_INFO,
            buf: Vec::new(),
        }
    }
    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> LineWriter {
        let level = match *meta.level() {
            tracing::Level::ERROR => LOG_ERROR,
            tracing::Level::WARN => LOG_WARNING,
            _ => LOG_INFO,
        };
        LineWriter {
            level,
            buf: Vec::new(),
        }
    }
}

/// Sets the C log callback and routes tracing into it. Only the first call counts.
///
/// # Safety
/// `cb` must stay callable from any thread for the life of the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_set_log(cb: LogCb) {
    if LOG.set(cb).is_ok() {
        let sub = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_target(true)
            .with_max_level(tracing::Level::INFO)
            .with_writer(MakeLineWriter)
            .finish();
        if tracing::subscriber::set_global_default(sub).is_err() {
            log(
                LOG_WARNING,
                "dante-input start: a tracing subscriber was already set; dlink's own lines are not forwarded",
            );
        }
    }
}

/// Lets blocks through to the plugin until closed. Closing waits, bounded, for a callback still
/// inside the plugin; no lock is held across the callback, so nothing it locks can deadlock it.
#[derive(Debug)]
pub struct Gate {
    open: AtomicBool,
    in_flight: AtomicU32,
}

/// One callback inside the gate; leaving drops the count.
pub struct Pass<'a>(&'a Gate);

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Gate {
    pub fn new() -> Arc<Gate> {
        Arc::new(Gate {
            open: AtomicBool::new(true),
            in_flight: AtomicU32::new(0),
        })
    }

    /// A pass while the gate is open. The count goes up before the check, so a close that sees
    /// no callback in flight sees every later one refused.
    pub fn enter(&self) -> Option<Pass<'_>> {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let pass = Pass(self);
        self.open.load(Ordering::SeqCst).then_some(pass)
    }

    /// Closes the gate and waits up to `wait` for the callbacks in flight; false if one is still
    /// inside after that.
    pub fn close(&self, wait: Duration) -> bool {
        self.open.store(false, Ordering::SeqCst);
        let deadline = Instant::now() + wait;
        while self.in_flight.load(Ordering::SeqCst) != 0 {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }
}

/// The C callback and its argument, sent to the feeder thread.
struct Target {
    cb: AudioCb,
    opaque: *mut c_void,
}

// SAFETY: the plugin's callback is thread-safe (it locks its own state) and `opaque` lives until
// dante_input_stop returns, which ends the feeder first.
unsafe impl Send for Target {}

/// The feed's counts: written by the pump and the feeder, read by the status thread.
#[derive(Debug, Default)]
pub struct Feed {
    /// Frames handed to OBS.
    pub delivered: AtomicU64,
    /// Frames the pump could not queue: the ring was full (the feeder a second behind).
    pub dropped: AtomicU64,
    /// The longest callback into OBS, ns.
    pub max_obs_ns: AtomicU64,
    /// Chunks whose play time was more than [`STAMP_JUMP_NS`] off its continuation (obs#53).
    pub stamp_jumps: AtomicU64,
}

/// A block's play time (dlink's `Block::play_at`, ns on OBS's clock) at its first frame's index in
/// the ring's frame count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub frame: u64,
    pub ns: i64,
}

/// The play times beside the ring, one per block that had one.
pub fn stamps() -> (rtrb::Producer<Stamp>, rtrb::Consumer<Stamp>) {
    rtrb::RingBuffer::new(STAMP_SLOTS)
}

/// The ring between the pump and the feeder: `channels` interleaved samples per frame,
/// `frames` frames.
pub fn ring(channels: usize, frames: usize) -> (rtrb::Producer<f32>, rtrb::Consumer<f32>) {
    rtrb::RingBuffer::new(channels.max(1) * frames.max(1))
}

/// The device's processor, on the pump thread: each block's f64 channels into the ring as
/// interleaved f32 frames. No lock, no allocation, no call out.
pub struct Sink {
    ring: rtrb::Producer<f32>,
    stamps: rtrb::Producer<Stamp>,
    /// Frames queued so far: the next block's first frame index.
    written: u64,
    channels: usize,
    feed: Arc<Feed>,
}

impl Sink {
    pub fn new(
        ring: rtrb::Producer<f32>,
        stamps: rtrb::Producer<Stamp>,
        channels: usize,
        feed: Arc<Feed>,
    ) -> Sink {
        Sink {
            ring,
            stamps,
            written: 0,
            channels,
            feed,
        }
    }
}

impl Process for Sink {
    fn process(&mut self, block: &mut Block<'_>) {
        let frames = block.frames();
        let n = self.channels;
        let have = block.inputs().min(n);
        let Ok(mut chunk) = self.ring.write_chunk_uninit(frames * n) else {
            self.feed
                .dropped
                .fetch_add(frames as u64, Ordering::Relaxed);
            return;
        };
        let (a, b) = chunk.as_mut_slices();
        for (i, slot) in a.iter_mut().chain(b.iter_mut()).enumerate() {
            let (f, c) = (i / n, i % n);
            let x = if c < have {
                block.input(c)[f] as f32
            } else {
                0.0
            };
            slot.write(x);
        }
        // SAFETY: every slot of the chunk was written above.
        unsafe { chunk.commit_all() };
        if let Some(ns) = block.play_at() {
            // A full stamp ring (a feeder a long way behind) loses this one: the feeder carries
            // the last play time on by the sample count until the next.
            let _ = self.stamps.push(Stamp {
                frame: self.written,
                ns,
            });
        }
        self.written += frames as u64;
    }
}

/// The device's processor: the RX into the ring ([`Sink`]) and, with TX, OBS's track out of the
/// TX FIFO ([`TxOut`]), each block.
pub struct Pump {
    sink: Sink,
    tx: Option<TxOut>,
}

impl Process for Pump {
    fn process(&mut self, block: &mut Block<'_>) {
        self.sink.process(block);
        if let Some(tx) = &mut self.tx {
            tx.fill(block);
        }
    }
}

/// The OBS side, on the `dante-input-obs` thread: the ring's whole frames to the C callback as
/// planes of at most [`MAX_CHUNK`] frames, behind the gate.
pub struct Feeder {
    target: Target,
    ring: rtrb::Consumer<f32>,
    stamps: rtrb::Consumer<Stamp>,
    /// The app rate of the ring's frames.
    rate: u32,
    /// Frames handed so far: the next chunk's first frame index.
    read: u64,
    /// The latest play time at or before `read`.
    anchor: Option<Stamp>,
    /// Where the last chunk's stamp continues (its stamp + its frames), for the jump check.
    continues: Option<u64>,
    planes: Vec<Vec<f32>>,
    ptrs: Vec<*const f32>,
    feed: Arc<Feed>,
    /// Closed by [`DanteInput::stop`]: no callback reaches the plugin after that.
    gate: Arc<Gate>,
}

// SAFETY: `ptrs` only ever points into `planes`, owned by the same value.
unsafe impl Send for Feeder {}

impl Feeder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cb: AudioCb,
        opaque: *mut c_void,
        ring: rtrb::Consumer<f32>,
        stamps: rtrb::Consumer<Stamp>,
        channels: usize,
        rate: u32,
        feed: Arc<Feed>,
        gate: Arc<Gate>,
    ) -> Feeder {
        Feeder {
            target: Target { cb, opaque },
            ring,
            stamps,
            rate: rate.max(1),
            read: 0,
            anchor: None,
            continues: None,
            planes: vec![vec![0.0; MAX_CHUNK]; channels],
            ptrs: vec![std::ptr::null(); channels],
            feed,
            gate,
        }
    }

    /// ns of `frames` frames at the app rate.
    fn frames_ns(&self, frames: u64) -> u64 {
        frames.saturating_mul(1_000_000_000) / u64::from(self.rate)
    }

    /// The play time of frame `read` (its block's, plus the frames before it in the block), 0
    /// without one; a jump off the last chunk's continuation is counted and logged (obs#53).
    fn stamp(&mut self, frames: usize) -> u64 {
        while let Ok(s) = self.stamps.peek() {
            if s.frame > self.read {
                break;
            }
            self.anchor = Some(*s);
            let _ = self.stamps.pop();
        }
        let Some(a) = self.anchor else {
            return 0;
        };
        let ns =
            a.ns.saturating_add(self.frames_ns(self.read - a.frame) as i64);
        let ts = u64::try_from(ns).unwrap_or(0);
        if let Some(cont) = self.continues {
            let off = ts.abs_diff(cont);
            if ts != 0 && off > STAMP_JUMP_NS {
                let jumps = self.feed.stamp_jumps.fetch_add(1, Ordering::Relaxed) + 1;
                if jumps <= 10 || jumps.is_multiple_of(100) {
                    log(
                        LOG_WARNING,
                        &format!(
                            "dante-input timing: a block's play time is {:.3} ms {} its continuation; OBS follows it (obs#53, {jumps} jump(s))",
                            off as f64 / 1e6,
                            if ts > cont { "after" } else { "before" },
                        ),
                    );
                }
            }
        }
        self.continues = (ts != 0).then(|| ts + self.frames_ns(frames as u64));
        ts
    }

    /// Hands every whole frame queued now to OBS; the frames handed, `None` once the gate is
    /// closed (nothing handed).
    pub fn drain(&mut self) -> Option<usize> {
        let n = self.planes.len();
        let mut handed = 0;
        loop {
            let frames = (self.ring.slots() / n.max(1)).min(MAX_CHUNK);
            if frames == 0 || n == 0 {
                return Some(handed);
            }
            // The guard holds its own handle on the gate, so `self` stays free for the stamp.
            let gate = Arc::clone(&self.gate);
            let pass = gate.enter()?;
            let Ok(chunk) = self.ring.read_chunk(frames * n) else {
                return Some(handed);
            };
            let (a, b) = chunk.as_slices();
            for (i, &x) in a.iter().chain(b).enumerate() {
                self.planes[i % n][i / n] = x;
            }
            chunk.commit_all();
            for (p, plane) in self.ptrs.iter_mut().zip(&self.planes) {
                *p = plane.as_ptr();
            }
            let ts = self.stamp(frames);
            self.read += frames as u64;
            let t = Instant::now();
            // SAFETY: the planes outlive the call; the callback copies them
            // (obs_source_output_audio).
            unsafe {
                (self.target.cb)(
                    self.target.opaque,
                    self.ptrs.as_ptr(),
                    n as u32,
                    frames as u32,
                    ts,
                )
            };
            let took = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
            drop(pass);
            self.feed.max_obs_ns.fetch_max(took, Ordering::Relaxed);
            self.feed
                .delivered
                .fetch_add(frames as u64, Ordering::Relaxed);
            handed += frames;
        }
    }
}

/// The feeder thread: drains every [`FEED_EVERY`] until stopped or the gate closes.
fn feed_loop(mut feeder: Feeder, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        if feeder.drain().is_none() {
            return;
        }
        std::thread::sleep(FEED_EVERY);
    }
}

/// The running device, its status thread and the stop signal.
pub struct DanteInput {
    link: Arc<Mutex<Option<Link>>>,
    channels: u32,
    rate: u32,
    feed: Arc<Feed>,
    gate: Arc<Gate>,
    stop: mpsc::Sender<()>,
    status: Option<JoinHandle<()>>,
    feed_stop: Arc<AtomicBool>,
    feeder: Option<JoinHandle<()>>,
    /// The TX's OBS side (obs#41); `None` without TX.
    tx: Option<Arc<TxIn>>,
}

impl DanteInput {
    /// Starts the device of the config at `path`, its RX blocks into `cb`.
    /// `obs_rate` is OBS's audio rate (0 when unknown): the blocks' rate unless the file sets one.
    pub fn start(
        path: &Path,
        obs_rate: u32,
        cb: AudioCb,
        opaque: *mut c_void,
    ) -> Result<DanteInput, String> {
        let cfg = InputConfig::load(path)?;
        let link_cfg = cfg.dlink(obs_rate);
        let rate = cfg.app_rate(obs_rate);
        log(
            LOG_INFO,
            &format!(
                "dante-input start: config={} name={} iface={} network_latency_ms={:?} receive_buffer_ms={:?} obs_rate={obs_rate} sample_rate={rate} device_rate={} converts={} rx={:?} tx={:?} tx_track={} ports={:?}/{:?}/{:?} rx_port_base={:?} tx_port_base={:?} subscriptions={} state_file={}",
                path.display(),
                cfg.name,
                cfg.iface,
                cfg.network_latency_ms,
                cfg.receive_buffer_ms,
                link_cfg.device_rate(),
                link_cfg.converts(),
                cfg.rx,
                cfg.tx,
                cfg.tx_track(),
                cfg.arc_port,
                cfg.cmc_port,
                cfg.flows_port,
                cfg.rx_port_base,
                cfg.tx_port_base,
                cfg.subscribe.len(),
                cfg.state_file.as_deref().unwrap_or(Path::new("")).display(),
            ),
        );
        let split = cfg
            .split(obs_rate)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        log(
            LOG_INFO,
            &format!(
                "dante-input start: latency split dante_latency_ns={} rx_path_ns={} tx_path_ns={}",
                split.latency_ns, split.rx_path_ns, split.tx_path_ns,
            ),
        );
        let n = cfg.rx.len();
        let feed = Arc::new(Feed::default());
        let gate = Gate::new();
        let rate_frames = usize::try_from(rate).unwrap_or(usize::MAX);
        let (producer, consumer) = ring(n, rate_frames.saturating_mul(RING_SECONDS));
        let (stamp_in, stamp_out) = stamps();
        let sink = Sink::new(producer, stamp_in, n, Arc::clone(&feed));
        let feeder = Feeder::new(
            cb,
            opaque,
            consumer,
            stamp_out,
            n,
            rate,
            Arc::clone(&feed),
            Arc::clone(&gate),
        );
        let (tx_in, tx_out) = match cfg.tx_track() {
            0 => (None, None),
            track => {
                let obs = cfg.obs_side_rate(obs_rate);
                let buffer = link_cfg
                    .bridge_buffer()
                    .ok_or("the TX has no app buffer (Config::app_buffer)")?;
                let (i, o) = tx::tx(track, obs, rate, buffer)?;
                log(
                    LOG_INFO,
                    &format!(
                        "dante-input start: TX of OBS track {track} on {:?}: OBS {obs} Hz into a FIFO held at {} ms (the app buffer) by dlink's ASRC player, read at {rate} Hz; tx_path_ns={} obs_to_wire_ns={}",
                        cfg.tx,
                        i.target_ms(),
                        split.tx_path_ns,
                        split.tx_path_ns + buffer.target_ns(),
                    ),
                );
                (Some(Arc::new(i)), Some(o))
            }
        };
        let map = Map {
            rx: (0..n).collect(),
            tx: (0..cfg.tx.len()).collect(),
        };
        let pump = Pump { sink, tx: tx_out };
        let link = Link::start(&link_cfg, &Place::Iface(cfg.iface.clone()), map, pump)
            .map_err(|e| format!("the device did not start: {e}"))?;
        log(
            LOG_INFO,
            &format!(
                "dante-input start: device {} running on {}, {n} RX channels, {} TX channels",
                cfg.name,
                cfg.iface,
                cfg.tx.len()
            ),
        );
        // dtray's obs line: the hand-off to OBS, on dlink's own status thread.
        let tray_feed = Arc::clone(&feed);
        let tray = Mutex::new(tray::Tray::new(rate, Instant::now()));
        link.set_app_status(move || {
            let counts = tray::Counts {
                delivered: tray_feed.delivered.load(Ordering::Relaxed),
                dropped: tray_feed.dropped.load(Ordering::Relaxed),
                max_obs_ns: tray_feed.max_obs_ns.load(Ordering::Relaxed),
            };
            match tray.lock() {
                Ok(mut t) => t.app_status(counts, Instant::now()),
                Err(_) => r#"{"line":"OBS: status lock poisoned","state":"error"}"#.to_owned(),
            }
        });
        log(
            LOG_INFO,
            "dante-input start: dtray shows the OBS hand-off (error on a dropped frame, warn after 2 s without audio)",
        );
        let link = Arc::new(Mutex::new(Some(link)));
        let (stop, stopped) = mpsc::channel();
        let watched = Arc::clone(&link);
        let counted = Arc::clone(&feed);
        let tx_counted = tx_in.clone();
        let spawned = std::thread::Builder::new()
            .name("dante-input-status".into())
            .spawn(move || status_loop(&watched, &stopped, &counted, tx_counted.as_deref()));
        let feed_stop = Arc::new(AtomicBool::new(false));
        let mut d = DanteInput {
            link,
            channels: n as u32,
            rate,
            feed,
            gate,
            stop,
            status: None,
            feed_stop: Arc::clone(&feed_stop),
            feeder: None,
            tx: tx_in,
        };
        match spawned {
            Ok(t) => d.status = Some(t),
            Err(e) => {
                // No device without its status line: stop it, the gate closed first.
                d.stop();
                return Err(format!("the status thread did not start: {e}"));
            }
        }
        let fed = std::thread::Builder::new()
            .name("dante-input-obs".into())
            .spawn(move || feed_loop(feeder, &feed_stop));
        match fed {
            Ok(t) => d.feeder = Some(t),
            Err(e) => {
                d.stop();
                return Err(format!("the OBS feeder thread did not start: {e}"));
            }
        }
        log(
            LOG_INFO,
            &format!(
                "dante-input start: the pump queues into a {RING_SECONDS} s ring; dante-input-obs hands it to OBS every {} ms",
                FEED_EVERY.as_millis()
            ),
        );
        Ok(d)
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// The device's sample rate, Hz: every block's.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The device's counts now; `None` once it is stopped.
    pub fn status(&self) -> Option<Status> {
        status_of(&self.link, &self.feed, self.tx.as_deref())
    }

    /// The OBS track the TX carries, from 1; 0 without TX.
    pub fn tx_track(&self) -> u32 {
        self.tx.as_ref().map_or(0, |t| t.track())
    }

    /// The rate OBS hands the TX track at, Hz; 0 without TX.
    pub fn tx_rate(&self) -> u32 {
        self.tx.as_ref().map_or(0, |t| t.rate())
    }

    /// OBS's track planes into the TX (OBS's audio thread only); nothing without TX.
    pub fn tx_push(&self, planes: &[&[f32]], frames: usize) {
        if let Some(t) = &self.tx {
            t.push(planes, frames);
        }
    }

    /// dlink's status JSON (what dtray reads), the obs part as `app`; `None` once stopped.
    pub fn device_status(&self) -> Option<String> {
        Some(self.link.lock().ok()?.as_ref()?.status())
    }

    /// Why the device ended by itself, if it did.
    pub fn ended(&self) -> Option<String> {
        self.link.lock().ok()?.as_ref()?.ended()
    }

    /// Closes the gate (no callback reaches the plugin after this; one in flight is waited for),
    /// ends the feeder, stops the status thread, then the device (its goodbyes sent).
    pub fn stop(mut self) {
        let closed = self.gate.close(GATE_WAIT);
        self.feed_stop.store(true, Ordering::Release);
        if closed {
            if let Some(t) = self.feeder.take() {
                let _ = t.join();
            }
        } else {
            // The feeder is stuck inside OBS: left to end on its own, it reaches OBS no more.
            log(
                LOG_ERROR,
                &format!(
                    "dante-input error: a block was still inside OBS {} ms after the stop; going on",
                    GATE_WAIT.as_millis()
                ),
            );
        }
        let _ = self.stop.send(());
        if let Some(t) = self.status.take() {
            let _ = t.join();
        }
        let link = self.link.lock().map(|mut l| l.take()).unwrap_or(None);
        match link {
            Some(link) => match link.stop() {
                dlink::Stopped::StillRunning => log(
                    LOG_ERROR,
                    "dante-input error: the device did not end within the stop wait; its pump is left \
                     to end (the gate is closed, so it reaches OBS no more)",
                ),
                stopped => log(
                    LOG_INFO,
                    &format!("dante-input stop: device stopped ({stopped:?})"),
                ),
            },
            None => log(LOG_INFO, "dante-input stop: no device was running"),
        }
    }
}

fn status_of(link: &Mutex<Option<Link>>, feed: &Feed, tx: Option<&TxIn>) -> Option<Status> {
    let guard = link.lock().ok()?;
    let l = guard.as_ref()?;
    let mut st = Status::read(l.media(), feed.delivered.load(Ordering::Relaxed));
    st.obs_dropped = feed.dropped.load(Ordering::Relaxed);
    st.max_obs_us = feed.max_obs_ns.load(Ordering::Relaxed) / 1_000;
    if let Some(t) = tx {
        st.tx(t.counts());
    }
    Some(st)
}

fn status_loop(
    link: &Mutex<Option<Link>>,
    stopped: &mpsc::Receiver<()>,
    feed: &Feed,
    tx: Option<&TxIn>,
) {
    let mut ended_logged = false;
    let mut lines = 0u32;
    loop {
        match stopped.recv_timeout(status_wait(lines)) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let Some(st) = status_of(link, feed, tx) else {
            return;
        };
        log(LOG_INFO, &st.line());
        lines = lines.saturating_add(1);
        let ended = link
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(Link::ended));
        if let Some(why) = ended
            && !ended_logged
        {
            log(
                LOG_ERROR,
                &format!("dante-input error: the device ended by itself: {why}"),
            );
            ended_logged = true;
        }
    }
}

fn write_err(err: *mut c_char, len: usize, msg: &str) {
    if err.is_null() || len == 0 {
        return;
    }
    let bytes = msg.as_bytes();
    let n = bytes.len().min(len - 1);
    // SAFETY: the caller gives `len` writable bytes at `err`.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), err.cast::<u8>(), n);
        *err.add(n) = 0;
    }
}

/// Starts the device of the config file at `path`. NULL on failure, the reason in `err`
/// (and logged as `dante-input error:`).
///
/// # Safety
/// `path` is a NUL-terminated string; `err` has `err_len` writable bytes or is NULL; `cb` and
/// `opaque` stay valid until dante_input_stop returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_start(
    path: *const c_char,
    obs_rate: u32,
    cb: AudioCb,
    opaque: *mut c_void,
    err: *mut c_char,
    err_len: usize,
) -> *mut DanteInput {
    if path.is_null() {
        write_err(err, err_len, "no config path");
        return std::ptr::null_mut();
    }
    // SAFETY: the caller's contract.
    let path = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    let started =
        std::panic::catch_unwind(|| DanteInput::start(Path::new(&path), obs_rate, cb, opaque));
    let msg = match started {
        Ok(Ok(d)) => return Box::into_raw(Box::new(d)),
        Ok(Err(why)) => why,
        Err(_) => "the device start panicked".to_string(),
    };
    log(LOG_ERROR, &format!("dante-input error: {msg}"));
    write_err(err, err_len, &msg);
    std::ptr::null_mut()
}

/// The device's sample rate, Hz: every block's.
///
/// # Safety
/// `d` comes from dante_input_start and was not stopped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_sample_rate(d: *const DanteInput) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { d.as_ref() }.map_or(0, DanteInput::rate)
}

/// The RX channels the device hands over per block.
///
/// # Safety
/// `d` comes from dante_input_start and was not stopped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_channels(d: *const DanteInput) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { d.as_ref() }.map_or(0, DanteInput::channels)
}

/// The OBS track (from 1) the device sends on its TX channels; 0 without TX (obs#41).
///
/// # Safety
/// `d` comes from dante_input_start and was not stopped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_tx_track(d: *const DanteInput) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { d.as_ref() }.map_or(0, DanteInput::tx_track)
}

/// The rate, Hz, to hand dante_input_tx_push the track at; 0 without TX.
///
/// # Safety
/// `d` comes from dante_input_start and was not stopped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_tx_rate(d: *const DanteInput) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { d.as_ref() }.map_or(0, DanteInput::tx_rate)
}

/// Hands the TX `frames` frames of OBS's track as `channels` float planes (left, right; one plane
/// goes out on both). Call it from OBS's audio thread only, at dante_input_tx_rate.
///
/// # Safety
/// `d` comes from dante_input_start and was not stopped; `planes` holds `channels` pointers, each
/// to `frames` floats, or is NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_tx_push(
    d: *const DanteInput,
    planes: *const *const f32,
    channels: u32,
    frames: u32,
) {
    // SAFETY: the caller's contract.
    let Some(d) = (unsafe { d.as_ref() }) else {
        return;
    };
    if planes.is_null() || channels == 0 {
        return;
    }
    let frames = frames as usize;
    let n = (channels as usize).min(config::TX_CHANNELS);
    let mut views: [&[f32]; config::TX_CHANNELS] = [&[]; config::TX_CHANNELS];
    for (c, view) in views.iter_mut().enumerate().take(n) {
        // SAFETY: the caller's contract: `channels` plane pointers.
        let p = unsafe { *planes.add(c) };
        if p.is_null() {
            return;
        }
        // SAFETY: the caller's contract: each plane holds `frames` floats.
        *view = unsafe { std::slice::from_raw_parts(p, frames) };
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        d.tx_push(&views[..n], frames)
    }));
}

/// Stops the device; no callback runs after this returns.
///
/// # Safety
/// `d` comes from dante_input_start, once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dante_input_stop(d: *mut DanteInput) {
    if d.is_null() {
        return;
    }
    // SAFETY: the caller's contract.
    let d = unsafe { Box::from_raw(d) };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.stop())).is_err() {
        log(LOG_ERROR, "dante-input error: the device stop panicked");
    }
}

#[cfg(test)]
mod tests;
