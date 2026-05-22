#[cfg(debug_assertions)]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        crate::debug_log::write(format_args!($($arg)*))
    };
}

#[cfg(not(debug_assertions))]
macro_rules! debug_log {
    ($($arg:tt)*) => {};
}

#[cfg(debug_assertions)]
static LOG_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(debug_assertions)]
pub(crate) fn quoted(value: impl std::fmt::Display) -> String {
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

#[cfg(debug_assertions)]
pub(crate) fn write(args: std::fmt::Arguments<'_>) {
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

#[cfg(debug_assertions)]
fn current_thread_id() -> String {
    let id = format!("{:?}", std::thread::current().id());
    id.strip_prefix("ThreadId(")
        .and_then(|id| id.strip_suffix(')'))
        .unwrap_or(&id)
        .to_owned()
}

#[cfg(debug_assertions)]
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

#[cfg(debug_assertions)]
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
