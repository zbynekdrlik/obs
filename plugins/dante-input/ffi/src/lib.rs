//! The C ABI under the OBS `dante_input` source (obs#1, camera-box#1485): one dlink device (dstack)
//! per OBS process, RX only. The pump's [`Sink`] only copies each block into a lock-free ring; the
//! `dante-input-obs` thread ([`Feeder`]) hands it to the C callback as float planes. OBS takes its
//! source mutexes in that call, so it never runs inside the pump's 0.33 ms block (v0.3.0 on
//! strih-snv: blocks of up to 6.7 ms, past the 5 ms margin, skipped).
//! The device's own log (dlink's clock choice, subscriptions, errors) and the 10 s status line go to
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

use config::InputConfig;
use status::Status;

/// OBS's log levels (util/base.h).
pub const LOG_ERROR: c_int = 100;
pub const LOG_WARNING: c_int = 200;
pub const LOG_INFO: c_int = 300;

/// The longest [`DanteInput::stop`] waits for a callback still inside the plugin.
pub const GATE_WAIT: Duration = Duration::from_secs(2);

/// How often the status line is logged (obs#1 log contract).
pub const STATUS_EVERY: Duration = Duration::from_secs(10);

/// How often the feeder hands the ring's frames to OBS.
pub const FEED_EVERY: Duration = Duration::from_millis(2);

/// The ring between the pump and the feeder holds this much audio; a feeder that far behind loses
/// frames (counted as `obs_dropped`).
pub const RING_SECONDS: usize = 1;

/// The most frames one callback hands OBS (10 ms at 48 kHz).
pub const MAX_CHUNK: usize = 480;

/// `planes[c]` holds `frames` samples of channel `c`, -1.0 to 1.0.
pub type AudioCb = unsafe extern "C" fn(
    opaque: *mut c_void,
    planes: *const *const f32,
    channels: u32,
    frames: u32,
);
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
    channels: usize,
    feed: Arc<Feed>,
}

impl Sink {
    pub fn new(ring: rtrb::Producer<f32>, channels: usize, feed: Arc<Feed>) -> Sink {
        Sink {
            ring,
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
    }
}

/// The OBS side, on the `dante-input-obs` thread: the ring's whole frames to the C callback as
/// planes of at most [`MAX_CHUNK`] frames, behind the gate.
pub struct Feeder {
    target: Target,
    ring: rtrb::Consumer<f32>,
    planes: Vec<Vec<f32>>,
    ptrs: Vec<*const f32>,
    feed: Arc<Feed>,
    /// Closed by [`DanteInput::stop`]: no callback reaches the plugin after that.
    gate: Arc<Gate>,
}

// SAFETY: `ptrs` only ever points into `planes`, owned by the same value.
unsafe impl Send for Feeder {}

impl Feeder {
    pub fn new(
        cb: AudioCb,
        opaque: *mut c_void,
        ring: rtrb::Consumer<f32>,
        channels: usize,
        feed: Arc<Feed>,
        gate: Arc<Gate>,
    ) -> Feeder {
        Feeder {
            target: Target { cb, opaque },
            ring,
            planes: vec![vec![0.0; MAX_CHUNK]; channels],
            ptrs: vec![std::ptr::null(); channels],
            feed,
            gate,
        }
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
            let pass = self.gate.enter()?;
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
            let t = Instant::now();
            // SAFETY: the planes outlive the call; the callback copies them
            // (obs_source_output_audio).
            unsafe {
                (self.target.cb)(
                    self.target.opaque,
                    self.ptrs.as_ptr(),
                    n as u32,
                    frames as u32,
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
                "dante-input start: config={} name={} iface={} latency_ms={} obs_rate={obs_rate} sample_rate={rate} device_rate={} converts={} rx={:?} ports={:?}/{:?}/{:?} rx_port_base={:?} subscriptions={} state_file={}",
                path.display(),
                cfg.name,
                cfg.iface,
                cfg.latency_ms,
                link_cfg.device_rate(),
                link_cfg.converts(),
                cfg.rx,
                cfg.arc_port,
                cfg.cmc_port,
                cfg.flows_port,
                cfg.rx_port_base,
                cfg.subscribe.len(),
                cfg.state_file.as_deref().unwrap_or(Path::new("")).display(),
            ),
        );
        let n = cfg.rx.len();
        let feed = Arc::new(Feed::default());
        let gate = Gate::new();
        let rate_frames = usize::try_from(rate).unwrap_or(usize::MAX);
        let (producer, consumer) = ring(n, rate_frames.saturating_mul(RING_SECONDS));
        let sink = Sink::new(producer, n, Arc::clone(&feed));
        let feeder = Feeder::new(
            cb,
            opaque,
            consumer,
            n,
            Arc::clone(&feed),
            Arc::clone(&gate),
        );
        let map = Map {
            rx: (0..n).collect(),
            tx: Vec::new(),
        };
        let link = Link::start(&link_cfg, &Place::Iface(cfg.iface.clone()), map, sink)
            .map_err(|e| format!("the device did not start: {e}"))?;
        log(
            LOG_INFO,
            &format!(
                "dante-input start: device {} running on {}, {n} RX channels",
                cfg.name, cfg.iface
            ),
        );
        let link = Arc::new(Mutex::new(Some(link)));
        let (stop, stopped) = mpsc::channel();
        let watched = Arc::clone(&link);
        let counted = Arc::clone(&feed);
        let spawned = std::thread::Builder::new()
            .name("dante-input-status".into())
            .spawn(move || status_loop(&watched, &stopped, &counted));
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
        status_of(&self.link, &self.feed)
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

fn status_of(link: &Mutex<Option<Link>>, feed: &Feed) -> Option<Status> {
    let guard = link.lock().ok()?;
    let l = guard.as_ref()?;
    let mut st = Status::read(l.media(), feed.delivered.load(Ordering::Relaxed));
    st.obs_dropped = feed.dropped.load(Ordering::Relaxed);
    st.max_obs_us = feed.max_obs_ns.load(Ordering::Relaxed) / 1_000;
    Some(st)
}

fn status_loop(link: &Mutex<Option<Link>>, stopped: &mpsc::Receiver<()>, feed: &Feed) {
    let mut ended_logged = false;
    loop {
        match stopped.recv_timeout(STATUS_EVERY) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let Some(st) = status_of(link, feed) else {
            return;
        };
        log(LOG_INFO, &st.line());
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
