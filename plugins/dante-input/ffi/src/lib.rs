//! The C ABI under the OBS `dante_input` source (obs#1, camera-box#1485): one dlink device (dstack)
//! per OBS process, RX only. Every block of its RX channels goes to the C callback as float planes.
//! The device's own log (dlink's clock choice, subscriptions, errors) and the 10 s status line go to
//! the C log callback, so they land in the OBS log.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use dlink::{Block, Link, Map, Place, Process};

pub mod config;
pub mod status;

use config::InputConfig;
use status::Status;

/// OBS's log levels (util/base.h).
pub const LOG_ERROR: c_int = 100;
pub const LOG_WARNING: c_int = 200;
pub const LOG_INFO: c_int = 300;

/// How often the status line is logged (obs#1 log contract).
pub const STATUS_EVERY: Duration = Duration::from_secs(10);

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

/// The C callback and its argument, sent to the pump thread.
struct Target {
    cb: AudioCb,
    opaque: *mut c_void,
}

// SAFETY: the plugin's callback is thread-safe (it locks its own state) and `opaque` lives until
// dante_input_stop returns, which stops the pump first.
unsafe impl Send for Target {}

/// The device's processor: f64 channel blocks to f32 planes for OBS.
pub struct Sink {
    target: Target,
    planes: Vec<Vec<f32>>,
    ptrs: Vec<*const f32>,
    delivered: Arc<AtomicU64>,
    /// Closed by [`DanteInput::stop`] before the device stops: no callback reaches the plugin
    /// after that, even when dlink leaves its pump running past the stop wait.
    gate: Arc<Mutex<bool>>,
}

// SAFETY: `ptrs` only ever points into `planes`, owned by the same value.
unsafe impl Send for Sink {}

impl Sink {
    fn new(
        cb: AudioCb,
        opaque: *mut c_void,
        channels: usize,
        delivered: Arc<AtomicU64>,
        gate: Arc<Mutex<bool>>,
    ) -> Sink {
        Sink {
            target: Target { cb, opaque },
            planes: vec![Vec::new(); channels],
            ptrs: vec![std::ptr::null(); channels],
            delivered,
            gate,
        }
    }
}

impl Process for Sink {
    fn process(&mut self, block: &mut Block<'_>) {
        // Held for the whole call, so closing the gate waits for a callback in flight.
        let Ok(open) = self.gate.lock() else { return };
        if !*open {
            return;
        }
        let frames = block.frames();
        let channels = block.inputs().min(self.planes.len());
        for c in 0..channels {
            let plane = &mut self.planes[c];
            // Grows once to the device's block; no allocation after that.
            if plane.len() != frames {
                plane.resize(frames, 0.0);
            }
            for (out, x) in plane.iter_mut().zip(block.input(c)) {
                *out = *x as f32;
            }
            self.ptrs[c] = plane.as_ptr();
        }
        // SAFETY: the planes outlive the call; the callback copies them (obs_source_output_audio).
        unsafe {
            (self.target.cb)(
                self.target.opaque,
                self.ptrs.as_ptr(),
                channels as u32,
                frames as u32,
            )
        };
        self.delivered.fetch_add(frames as u64, Ordering::Relaxed);
    }
}

/// The running device, its status thread and the stop signal.
pub struct DanteInput {
    link: Arc<Mutex<Option<Link>>>,
    channels: u32,
    delivered: Arc<AtomicU64>,
    gate: Arc<Mutex<bool>>,
    stop: mpsc::Sender<()>,
    status: Option<JoinHandle<()>>,
}

impl DanteInput {
    /// Starts the device of the config at `path`, its RX blocks into `cb`.
    pub fn start(path: &Path, cb: AudioCb, opaque: *mut c_void) -> Result<DanteInput, String> {
        let cfg = InputConfig::load(path)?;
        log(
            LOG_INFO,
            &format!(
                "dante-input start: config={} name={} iface={} latency_ms={} rx={:?} ports={:?}/{:?}/{:?} rx_port_base={:?} subscriptions={} state_file={}",
                path.display(),
                cfg.name,
                cfg.iface,
                cfg.latency_ms,
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
        let delivered = Arc::new(AtomicU64::new(0));
        let gate = Arc::new(Mutex::new(true));
        let sink = Sink::new(cb, opaque, n, Arc::clone(&delivered), Arc::clone(&gate));
        let map = Map {
            rx: (0..n).collect(),
            tx: Vec::new(),
        };
        let link = Link::start(&cfg.dlink(), &Place::Iface(cfg.iface.clone()), map, sink)
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
        let counted = Arc::clone(&delivered);
        let spawned = std::thread::Builder::new()
            .name("dante-input-status".into())
            .spawn(move || status_loop(&watched, &stopped, &counted));
        let mut d = DanteInput {
            link,
            channels: n as u32,
            delivered,
            gate,
            stop,
            status: None,
        };
        match spawned {
            Ok(t) => d.status = Some(t),
            Err(e) => {
                // No device without its status line: stop it, the gate closed first.
                d.stop();
                return Err(format!("the status thread did not start: {e}"));
            }
        }
        Ok(d)
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// The device's counts now; `None` once it is stopped.
    pub fn status(&self) -> Option<Status> {
        status_of(&self.link, &self.delivered)
    }

    /// Why the device ended by itself, if it did.
    pub fn ended(&self) -> Option<String> {
        self.link.lock().ok()?.as_ref()?.ended()
    }

    /// Closes the gate (no callback reaches the plugin after this; one in flight is waited for),
    /// stops the status thread, then the device (its goodbyes sent).
    pub fn stop(mut self) {
        close_gate(&self.gate);
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

fn close_gate(gate: &Mutex<bool>) {
    match gate.lock() {
        Ok(mut open) => *open = false,
        Err(poisoned) => *poisoned.into_inner() = false,
    }
}

fn status_of(link: &Mutex<Option<Link>>, delivered: &AtomicU64) -> Option<Status> {
    let guard = link.lock().ok()?;
    let l = guard.as_ref()?;
    Some(Status::read(l.media(), delivered.load(Ordering::Relaxed)))
}

fn status_loop(link: &Mutex<Option<Link>>, stopped: &mpsc::Receiver<()>, delivered: &AtomicU64) {
    let mut ended_logged = false;
    loop {
        match stopped.recv_timeout(STATUS_EVERY) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let Some(st) = status_of(link, delivered) else {
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
    let started = std::panic::catch_unwind(|| DanteInput::start(Path::new(&path), cb, opaque));
    let msg = match started {
        Ok(Ok(d)) => return Box::into_raw(Box::new(d)),
        Ok(Err(why)) => why,
        Err(_) => "the device start panicked".to_string(),
    };
    log(LOG_ERROR, &format!("dante-input error: {msg}"));
    write_err(err, err_len, &msg);
    std::ptr::null_mut()
}

/// The sample rate of every block: dlink's (the config sets none).
#[unsafe(no_mangle)]
pub extern "C" fn dante_input_sample_rate() -> u32 {
    dlink::SAMPLE_RATE
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
