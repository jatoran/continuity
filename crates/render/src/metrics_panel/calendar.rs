//! Calendar helpers for metrics-panel layout.

pub(super) fn short_month_day(iso: &str) -> String {
    let parts: Vec<&str> = iso.split('-').collect();
    if parts.len() != 3 {
        return iso.to_string();
    }
    let month = parts[1].parse::<usize>().unwrap_or(0);
    let day = parts[2].parse::<u32>().unwrap_or(0);
    const MONTHS: [&str; 13] = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    match MONTHS.get(month).copied().filter(|m| !m.is_empty()) {
        Some(name) if day > 0 => format!("{name} {day}"),
        _ => iso.to_string(),
    }
}

pub(super) fn weekday_index_from_iso(iso: &str) -> Option<usize> {
    let mut parts = iso.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let days = days_from_civil(y, m, d)?;
    // 1970-01-01 was Thursday. Sunday = 0.
    Some((days + 4).rem_euclid(7) as usize)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = i64::from(year) - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(month) + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}
