//! The obs line in dtray (obs#1, camera-box#1485): what happens after the device, the hand-off to
//! OBS. dtray shows the device's own line (clock, flows) from `Link::status`; this adds the
//! `app` part dlink asks for through `Link::set_app_status`: `{"line": .., "state": ..}`.
//!
//! - `error`: frames were dropped before OBS (the ring was full), i.e. audio lost.
//! - `warn`: no frame reached OBS for [`STALL`]: no audio from the device, or OBS holds the
//!   callback.
//! - `ok` otherwise.
//!
//! dlink calls the hook on a thread of its own, at most once a second and only while the status
//! is read, so nothing here delays the device.

use std::time::{Duration, Instant};

/// No frame to OBS for this long: `warn`.
pub const STALL: Duration = Duration::from_secs(2);

/// The feed's numbers at one read.
#[derive(Debug, Clone, Copy)]
pub struct Counts {
    pub delivered: u64,
    pub dropped: u64,
    pub max_obs_ns: u64,
}

/// What the last read saw: `delivered` and when it last moved.
#[derive(Debug)]
pub struct Tray {
    rate: u32,
    delivered: u64,
    moved: Instant,
}

impl Tray {
    /// A tray for OBS at `rate` Hz, started at `now`.
    pub fn new(rate: u32, now: Instant) -> Self {
        Tray {
            rate: rate.max(1),
            delivered: 0,
            moved: now,
        }
    }

    /// The app status JSON for `c` read at `now`.
    pub fn app_status(&mut self, c: Counts, now: Instant) -> String {
        if c.delivered != self.delivered {
            self.delivered = c.delivered;
            self.moved = now;
        }
        let still = now.saturating_duration_since(self.moved);
        let secs = c.delivered / u64::from(self.rate);
        let hold_ms = c.max_obs_ns as f64 / 1e6;
        let (state, line) = if c.dropped > 0 {
            (
                "error",
                format!(
                    "OBS: {} frames dropped before OBS, {secs} s delivered",
                    c.dropped
                ),
            )
        } else if still >= STALL {
            (
                "warn",
                format!(
                    "OBS: no audio for {} s, {secs} s delivered",
                    still.as_secs()
                ),
            )
        } else {
            (
                "ok",
                format!("OBS: {secs} s delivered, 0 dropped, hold max {hold_ms:.1} ms"),
            )
        };
        serde_json::json!({ "line": line, "state": state }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(json: &str) -> (String, String) {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        (
            v["state"].as_str().unwrap().to_owned(),
            v["line"].as_str().unwrap().to_owned(),
        )
    }

    fn counts(delivered: u64, dropped: u64) -> Counts {
        Counts {
            delivered,
            dropped,
            max_obs_ns: 6_773_000,
        }
    }

    #[test]
    fn audio_flowing_to_obs_is_ok_with_the_seconds_and_the_longest_hold() {
        let t0 = Instant::now();
        let mut tray = Tray::new(48_000, t0);
        let (state, line) =
            read(&tray.app_status(counts(48_000 * 90, 0), t0 + Duration::from_secs(1)));
        assert_eq!(state, "ok");
        assert_eq!(line, "OBS: 90 s delivered, 0 dropped, hold max 6.8 ms");
    }

    #[test]
    fn a_dropped_frame_is_an_error_even_while_audio_flows() {
        let t0 = Instant::now();
        let mut tray = Tray::new(48_000, t0);
        let (state, line) = read(&tray.app_status(counts(48_000 * 5, 32), t0));
        assert_eq!(state, "error");
        assert_eq!(line, "OBS: 32 frames dropped before OBS, 5 s delivered");
    }

    #[test]
    fn no_frame_to_obs_for_the_stall_is_a_warning_and_audio_again_clears_it() {
        let t0 = Instant::now();
        let mut tray = Tray::new(48_000, t0);
        assert_eq!(read(&tray.app_status(counts(480, 0), t0)).0, "ok");
        let just = t0 + STALL - Duration::from_millis(1);
        assert_eq!(read(&tray.app_status(counts(480, 0), just)).0, "ok");
        let (state, line) =
            read(&tray.app_status(counts(480, 0), t0 + STALL + Duration::from_secs(1)));
        assert_eq!(state, "warn");
        assert_eq!(line, "OBS: no audio for 3 s, 0 s delivered");
        let later = t0 + Duration::from_secs(4);
        assert_eq!(read(&tray.app_status(counts(960, 0), later)).0, "ok");
    }

    #[test]
    fn a_device_that_never_delivered_warns_after_the_stall() {
        let t0 = Instant::now();
        let mut tray = Tray::new(48_000, t0);
        assert_eq!(
            read(&tray.app_status(counts(0, 0), t0 + Duration::from_secs(1))).0,
            "ok"
        );
        assert_eq!(read(&tray.app_status(counts(0, 0), t0 + STALL)).0, "warn");
    }

    #[test]
    fn the_status_is_one_json_object_well_under_dlink_s_4096_bytes() {
        let mut tray = Tray::new(48_000, Instant::now());
        let json = tray.app_status(
            Counts {
                delivered: u64::MAX,
                dropped: u64::MAX,
                max_obs_ns: u64::MAX,
            },
            Instant::now(),
        );
        assert!(json.len() < 4096, "{}", json.len());
        assert!(
            serde_json::from_str::<serde_json::Value>(&json)
                .unwrap()
                .is_object()
        );
    }
}
