#[cfg(debug_assertions)]
mod flip;
mod lifecycle;
#[cfg(not(test))]
mod misc;
mod present;

#[cfg(debug_assertions)]
pub(crate) use flip::*;
pub(crate) use lifecycle::*;
#[cfg(not(test))]
pub(crate) use misc::*;
pub(crate) use present::*;

#[cfg(debug_assertions)]
use std::collections::BTreeMap;
#[cfg(debug_assertions)]
use std::sync::{Mutex, OnceLock};

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SampleDecision {
    pub should_log: bool,
    pub count: u64,
}

#[cfg(debug_assertions)]
pub(crate) struct Limiter<K> {
    counts: BTreeMap<K, u64>,
}

#[cfg(debug_assertions)]
impl<K> Default for Limiter<K> {
    fn default() -> Self {
        Self {
            counts: BTreeMap::new(),
        }
    }
}

#[cfg(debug_assertions)]
impl<K: Ord> Limiter<K> {
    pub(crate) fn sample(&mut self, key: K, interval: u64) -> SampleDecision {
        let count = self.counts.entry(key).or_insert(0);
        *count = count.saturating_add(1);
        let count = *count;
        SampleDecision {
            should_log: count == 1 || count.is_multiple_of(interval),
            count,
        }
    }
}

#[cfg(debug_assertions)]
pub(crate) struct SharedLimiter<K> {
    interval: u64,
    limiter: OnceLock<Mutex<Limiter<K>>>,
}

#[cfg(debug_assertions)]
impl<K: Ord> SharedLimiter<K> {
    pub(crate) const fn new(interval: u64) -> Self {
        Self {
            interval,
            limiter: OnceLock::new(),
        }
    }

    pub(crate) fn sample(&self, key: K) -> SampleDecision {
        self.limiter
            .get_or_init(|| Mutex::new(Limiter::default()))
            .lock()
            .map(|mut limiter| limiter.sample(key, self.interval))
            .unwrap_or(SampleDecision {
                should_log: true,
                count: 1,
            })
    }
}

#[cfg(all(debug_assertions, not(test)))]
static LOG_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(debug_assertions)]
fn quoted(value: impl std::fmt::Display) -> String {
    let value = value.to_string();
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(all(debug_assertions, not(test)))]
fn write(args: std::fmt::Arguments<'_>) {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::sync::atomic::Ordering;

    let log_dir = std::env::temp_dir().join("dwm-lut-rs");
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }

    let log_path = log_dir.join("hook-debug.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };

    let timestamp = utc_timestamp();
    let pid = std::process::id();
    let tid = current_thread_id();
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let _ = writeln!(
        file,
        "dwm_lut_hook ts={} pid={pid} tid={tid} seq={seq} {args}",
        quoted(timestamp)
    );
}

#[cfg(all(debug_assertions, test))]
fn write(_args: std::fmt::Arguments<'_>) {}

#[cfg(all(debug_assertions, not(test)))]
fn current_thread_id() -> String {
    let id = format!("{:?}", std::thread::current().id());
    id.strip_prefix("ThreadId(")
        .and_then(|id| id.strip_suffix(')'))
        .unwrap_or(&id)
        .to_owned()
}

#[cfg(all(debug_assertions, not(test)))]
fn utc_timestamp() -> String {
    const SECONDS_PER_DAY: u64 = 86_400;

    let Ok(duration) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return "1970-01-01T00:00:00.000Z".to_owned();
    };

    let total_seconds = duration.as_secs();
    let millis = duration.subsec_millis();
    let days = total_seconds / SECONDS_PER_DAY;
    let seconds_of_day = total_seconds % SECONDS_PER_DAY;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[cfg(all(debug_assertions, not(test)))]
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };

    (year + i64::from(month <= 2), month as u32, day as u32)
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::Limiter;

    #[test]
    fn limiter_emits_first_and_every_interval() {
        let mut limiter = Limiter::default();
        let interval = 300;
        let decision = limiter.sample(1u8, interval);
        assert!(decision.should_log);
        assert_eq!(decision.count, 1);
        for expected in 2..interval {
            let decision = limiter.sample(1u8, interval);
            assert!(!decision.should_log);
            assert_eq!(decision.count, expected);
        }
        let decision = limiter.sample(1u8, interval);
        assert!(decision.should_log);
        assert_eq!(decision.count, interval);
        let decision = limiter.sample(1u8, interval);
        assert!(!decision.should_log);
        assert_eq!(decision.count, interval + 1);
    }

    #[test]
    fn limiter_tracks_keys_independently() {
        let mut limiter = Limiter::default();
        assert!(limiter.sample(1u8, 300).should_log);
        assert!(limiter.sample(2u8, 300).should_log);
        assert!(!limiter.sample(1u8, 300).should_log);
        assert!(!limiter.sample(2u8, 300).should_log);
    }
}
