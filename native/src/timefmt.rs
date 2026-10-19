//! Local-time helpers (libc, no date crate) and the short formats the UI shows.

use crate::session::now_ms;

pub const DAY_MS: i64 = 86_400_000;

fn local_tm(ms: i64) -> libc::tm {
    // SAFETY: localtime_r only writes into the struct we pass it.
    unsafe {
        let t = (ms.div_euclid(1000)) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

fn mktime(mut tm: libc::tm) -> i64 {
    tm.tm_isdst = -1;
    // SAFETY: mktime reads/normalises the struct we own.
    unsafe { libc::mktime(&mut tm) as i64 * 1000 }
}

/// Local midnight at the start of the day containing `ms`.
pub fn start_of_day(ms: i64) -> i64 {
    let mut tm = local_tm(ms);
    tm.tm_hour = 0;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    mktime(tm)
}

/// "2026-10-08" → end of that local day (23:59:59.999), epoch ms.
pub fn parse_ymd_end_of_day(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: i32 = it.next()?.parse().ok()?;
    let d: i32 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut tm = local_tm(now_ms());
    tm.tm_year = y - 1900;
    tm.tm_mon = m - 1;
    tm.tm_mday = d;
    tm.tm_hour = 23;
    tm.tm_min = 59;
    tm.tm_sec = 59;
    Some(mktime(tm) + 999)
}

pub fn to_ymd(ms: i64) -> String {
    let tm = local_tm(ms);
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// "Today", "Yesterday", "Tomorrow", "Thu", or "Oct 8".
pub fn fmt_day(ms: i64, now: i64) -> String {
    let diff = ((start_of_day(ms) - start_of_day(now)) as f64 / DAY_MS as f64).round() as i64;
    match diff {
        0 => "Today".into(),
        -1 => "Yesterday".into(),
        1 => "Tomorrow".into(),
        2..=6 => WEEKDAYS[local_tm(ms).tm_wday.clamp(0, 6) as usize].into(),
        _ => {
            let tm = local_tm(ms);
            format!("{} {}", MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_mday)
        }
    }
}

pub fn fmt_time(ms: i64) -> String {
    let tm = local_tm(ms);
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

pub fn fmt_stamp(ms: i64, now: i64) -> String {
    format!("{} {}", fmt_day(ms, now), fmt_time(ms))
}

/// "<1m", "45m", "3h 5m", "2d 4h".
pub fn fmt_duration(ms: i64) -> String {
    let m = (ms / 60_000).max(0);
    if m < 1 {
        return "<1m".into();
    }
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 {
        return if m % 60 > 0 { format!("{h}h {}m", m % 60) } else { format!("{h}h") };
    }
    let d = h / 24;
    if h % 24 > 0 {
        format!("{d}d {}h", h % 24)
    } else {
        format!("{d}d")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ymd_round_trip_and_days() {
        let due = parse_ymd_end_of_day("2026-10-08").unwrap();
        assert_eq!(to_ymd(due), "2026-10-08");
        assert!(parse_ymd_end_of_day("2026-13-01").is_none());
        assert!(parse_ymd_end_of_day("nope").is_none());
        let now = now_ms();
        assert_eq!(fmt_day(now, now), "Today");
        assert_eq!(fmt_day(now - DAY_MS, now), "Yesterday");
        assert_eq!(fmt_duration(3 * 3_600_000 + 5 * 60_000), "3h 5m");
        assert_eq!(fmt_duration(2 * DAY_MS), "2d");
    }
}
