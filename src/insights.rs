//! Frontend-independent logic: battery history, statistics and notification decisions.
//! Pure functions plus a small JSONL store, so every UI behaves the same. No Bluetooth here.
use crate::protocol::Battery;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

/// History is a rolling window.
pub const KEEP_SECS: f64 = 7.0 * 24.0 * 3600.0;
/// Unchanged values are re-sampled at most this often.
pub const RESAMPLE_SECS: f64 = 300.0;
const PRUNE_EVERY_SECS: f64 = 3600.0;
/// Gaps longer than this between samples are ignored when computing drain/charge time.
const MAX_GAP_SECS: f64 = 1800.0;

fn is_false(b: &bool) -> bool {
    !*b
}

/// One history line. Same JSON as the Swift app's `history.jsonl` (`t,l,r,c,lc,rc,cc`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub t: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<u8>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub lc: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub rc: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cc: bool,
}

impl Sample {
    pub fn from_battery(b: &Battery, t: f64) -> Option<Sample> {
        if b.left.is_none() && b.right.is_none() && b.case.is_none() {
            return None;
        }
        Some(Sample {
            t,
            l: b.left.map(|c| c.percent),
            r: b.right.map(|c| c.percent),
            c: b.case.map(|c| c.percent),
            lc: b.left.is_some_and(|c| c.charging),
            rc: b.right.is_some_and(|c| c.charging),
            cc: b.case.is_some_and(|c| c.charging),
        })
    }

    /// `side` is "left", "right" or "case".
    pub fn level(&self, side: &str) -> Option<(u8, bool)> {
        match side {
            "left" => self.l.map(|p| (p, self.lc)),
            "right" => self.r.map(|p| (p, self.rc)),
            _ => self.c.map(|p| (p, self.cc)),
        }
    }

    fn same_values(&self, o: &Sample) -> bool {
        (self.l, self.r, self.c, self.lc, self.rc, self.cc) == (o.l, o.r, o.c, o.lc, o.rc, o.cc)
    }
}

/// Rolling 7-day battery history, persisted as JSONL.
pub struct History {
    path: Option<PathBuf>,
    pub samples: Vec<Sample>,
    last_prune: f64,
}

impl History {
    pub fn in_memory() -> Self {
        Self { path: None, samples: Vec::new(), last_prune: 0.0 }
    }

    /// Loads `path` (missing file = empty) and prunes it.
    pub fn load(path: PathBuf, now: f64) -> Self {
        let samples = std::fs::read_to_string(&path)
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default();
        let mut h = Self { path: Some(path), samples, last_prune: now };
        h.prune(now, true);
        h
    }

    fn prune(&mut self, now: f64, force: bool) {
        self.last_prune = now;
        let cutoff = now - KEEP_SECS;
        if !self.samples.first().is_some_and(|s| s.t < cutoff) && !force {
            return;
        }
        let before = self.samples.len();
        self.samples.retain(|s| s.t >= cutoff);
        if self.samples.len() != before {
            self.rewrite();
        }
    }

    fn rewrite(&self) {
        let Some(path) = &self.path else { return };
        let mut text = String::new();
        for s in &self.samples {
            if let Ok(l) = serde_json::to_string(s) {
                text.push_str(&l);
                text.push('\n');
            }
        }
        let _ = std::fs::write(path, text);
    }

    /// Returns true when a sample was stored.
    pub fn record(&mut self, b: &Battery, now: f64) -> bool {
        let Some(s) = Sample::from_battery(b, now) else { return false };
        if now - self.last_prune > PRUNE_EVERY_SECS {
            self.prune(now, false);
        }
        if let Some(last) = self.samples.last() {
            if last.same_values(&s) && now - last.t < RESAMPLE_SECS {
                return false;
            }
        }
        self.samples.push(s);
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let (Ok(line), Ok(mut f)) = (serde_json::to_string(&s), std::fs::OpenOptions::new().create(true).append(true).open(path)) {
                let _ = writeln!(f, "{line}");
            }
        }
        true
    }
}

#[derive(Debug, PartialEq)]
pub struct Stats {
    pub side: String,
    pub hours: u32,
    pub from: f64,
    pub to: f64,
    pub bucket: f64,
    /// (time, percent, charging), one per time bucket, oldest first.
    pub points: Vec<(f64, u8, bool)>,
    /// Charging periods (start, end).
    pub spans: Vec<(f64, f64)>,
    pub drain_per_hour: Option<f64>,
    pub est_life_secs: Option<f64>,
    pub charge_secs: f64,
    pub average: Option<f64>,
}

pub fn compute_stats(samples: &[Sample], side: &str, hours: u32, now: f64) -> Stats {
    let from = now - hours as f64 * 3600.0;
    let bucket = hours as f64 * 3600.0 / 96.0;
    let pts: Vec<(f64, u8, bool)> = samples.iter().filter(|s| s.t >= from).filter_map(|s| s.level(side).map(|(p, c)| (s.t, p, c))).collect();

    let mut by_bucket: BTreeMap<i64, (f64, u8, bool)> = BTreeMap::new();
    for &p in &pts {
        by_bucket.insert((p.0 / bucket) as i64, p);
    }
    let points: Vec<_> = by_bucket.into_values().collect();

    let mut spans = Vec::new();
    let (mut start, mut prev): (Option<f64>, Option<f64>) = (None, None);
    for &(t, _, charging) in &points {
        if charging {
            start.get_or_insert(t);
            prev = Some(t);
        } else if let (Some(st), Some(pr)) = (start.take(), prev) {
            spans.push((st, pr + bucket));
        }
    }
    if let (Some(st), Some(pr)) = (start, prev) {
        spans.push((st, pr + bucket));
    }

    let (mut drop, mut hrs, mut charge_secs) = (0.0, 0.0, 0.0);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let dt = b.0 - a.0;
        if dt <= 0.0 || dt > MAX_GAP_SECS {
            continue;
        }
        if !a.2 && !b.2 {
            drop += a.1.saturating_sub(b.1) as f64;
            hrs += dt / 3600.0;
        } else if a.2 && b.2 {
            charge_secs += dt;
        }
    }
    let drain_per_hour = (hrs >= 0.25 && drop > 0.0).then(|| drop / hrs);
    let average = (!pts.is_empty()).then(|| pts.iter().map(|p| p.1 as f64).sum::<f64>() / pts.len() as f64);
    Stats {
        side: side.to_string(),
        hours,
        from,
        to: now,
        bucket,
        points,
        spans,
        drain_per_hour,
        est_life_secs: drain_per_hour.map(|d| 100.0 / d * 3600.0),
        charge_secs,
        average,
    }
}

impl Stats {
    pub fn to_json(&self) -> Value {
        json!({
            "stats": {
                "side": self.side,
                "hours": self.hours,
                "from": self.from,
                "to": self.to,
                "bucket": self.bucket,
                "points": self.points.iter().map(|p| json!([p.0, p.1, p.2])).collect::<Vec<_>>(),
                "spans": self.spans.iter().map(|s| json!([s.0, s.1])).collect::<Vec<_>>(),
                "drain_per_hour": self.drain_per_hour,
                "est_life_secs": self.est_life_secs,
                "charge_secs": self.charge_secs,
                "average": self.average,
            }
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum Notice {
    Connected,
    Disconnected,
    LowBattery { cell: &'static str, percent: u8 },
}

impl Notice {
    pub fn to_json(&self) -> Value {
        match self {
            Notice::Connected => json!({"event": "connected"}),
            Notice::Disconnected => json!({"event": "disconnected"}),
            Notice::LowBattery { cell, percent } => json!({"event": "low_battery", "cell": cell, "percent": percent}),
        }
    }
}

/// Decides when a UI should show a notification. Frontends only display the result.
pub struct Notifier {
    pub low_enabled: bool,
    pub threshold: u8,
    pub conn_enabled: bool,
    prev_connected: Option<bool>,
    alerted: [bool; 3],
}

impl Default for Notifier {
    fn default() -> Self {
        Self { low_enabled: true, threshold: 20, conn_enabled: false, prev_connected: None, alerted: [false; 3] }
    }
}

impl Notifier {
    /// A cell alerts once at or below the threshold while not charging, and re-arms when it
    /// charges or climbs above threshold + 5.
    pub fn evaluate(&mut self, connected: bool, bat: Option<&Battery>) -> Vec<Notice> {
        let mut out = Vec::new();
        if let Some(prev) = self.prev_connected {
            if self.conn_enabled && prev != connected {
                out.push(if connected { Notice::Connected } else { Notice::Disconnected });
            }
        }
        self.prev_connected = Some(connected);
        if !connected {
            return out;
        }
        let Some(b) = bat else { return out };
        for (i, (name, cell)) in [("left", b.left), ("right", b.right), ("case", b.case)].into_iter().enumerate() {
            let Some(c) = cell else { continue };
            if c.charging || c.percent > self.threshold.saturating_add(5) {
                self.alerted[i] = false;
            } else if !self.alerted[i] && c.percent <= self.threshold && !c.charging {
                self.alerted[i] = true;
                if self.low_enabled {
                    out.push(Notice::LowBattery { cell: name, percent: c.percent });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Cell;

    fn bat(l: Option<(u8, bool)>, r: Option<(u8, bool)>, c: Option<(u8, bool)>) -> Battery {
        let cell = |x: Option<(u8, bool)>| x.map(|(percent, charging)| Cell { percent, charging });
        Battery { left: cell(l), right: cell(r), case: cell(c) }
    }

    #[test]
    fn records_dedupes_and_resamples() {
        let mut h = History::in_memory();
        let b = bat(Some((80, false)), Some((79, false)), None);
        assert!(h.record(&b, 1000.0));
        assert!(!h.record(&b, 1100.0)); // same values, too soon
        assert!(h.record(&b, 1400.0)); // same values, past 5 minutes
        assert!(h.record(&bat(Some((79, false)), Some((79, false)), None), 1401.0));
        assert!(!h.record(&Battery::default(), 2000.0)); // nothing to store
    }

    #[test]
    fn sample_json_matches_swift_format() {
        let s = Sample::from_battery(&bat(Some((50, true)), None, Some((30, false))), 12.5).unwrap();
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"t":12.5,"l":50,"c":30,"lc":true}"#);
        let back: Sample = serde_json::from_str(r#"{"t":1,"l":9,"lc":false,"rc":false,"cc":false}"#).unwrap();
        assert_eq!(back.l, Some(9));
        assert_eq!(back.r, None);
    }

    #[test]
    fn stats_drain_and_charge() {
        let now = 100_000.0;
        let mk = |t: f64, p: u8, ch: bool| Sample { t, l: Some(p), r: None, c: None, lc: ch, rc: false, cc: false };
        // 20 points over 1h at 300s spacing, losing 1% each time = 12%/h
        let mut v: Vec<Sample> = (0..13).map(|i| mk(now - 4000.0 + i as f64 * 300.0, 90 - i as u8, false)).collect();
        v.push(mk(now - 100.0, 40, true));
        v.push(mk(now - 50.0, 41, true));
        let s = compute_stats(&v, "left", 24, now);
        assert!((s.drain_per_hour.unwrap() - 12.0).abs() < 0.01);
        assert!((s.est_life_secs.unwrap() - 100.0 / 12.0 * 3600.0).abs() < 1.0);
        assert_eq!(s.charge_secs, 50.0);
        assert_eq!(s.spans.len(), 1);
        assert!(s.average.unwrap() > 40.0);
        assert!(compute_stats(&v, "right", 24, now).points.is_empty());
    }

    #[test]
    fn low_battery_alerts_once_and_rearms() {
        let mut n = Notifier::default();
        assert!(n.evaluate(true, Some(&bat(Some((50, false)), None, None))).is_empty());
        assert_eq!(n.evaluate(true, Some(&bat(Some((20, false)), None, None))), vec![Notice::LowBattery { cell: "left", percent: 20 }]);
        assert!(n.evaluate(true, Some(&bat(Some((19, false)), None, None))).is_empty()); // already alerted
        assert!(n.evaluate(true, Some(&bat(Some((23, false)), None, None))).is_empty()); // not past threshold + 5
        assert!(n.evaluate(true, Some(&bat(Some((19, false)), None, None))).is_empty()); // still not re-armed
        assert!(n.evaluate(true, Some(&bat(Some((19, true)), None, None))).is_empty()); // charging re-arms
        assert_eq!(n.evaluate(true, Some(&bat(Some((18, false)), None, None))).len(), 1);
    }

    #[test]
    fn connection_notices_skip_startup() {
        let mut n = Notifier { conn_enabled: true, ..Default::default() };
        assert!(n.evaluate(false, None).is_empty()); // startup state is not a disconnect
        assert_eq!(n.evaluate(true, None), vec![Notice::Connected]);
        assert_eq!(n.evaluate(false, None), vec![Notice::Disconnected]);
        n.conn_enabled = false;
        assert!(n.evaluate(true, None).is_empty());
    }
}
