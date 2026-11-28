//! Wall clock with an optional frozen override for deterministic tests.
//!
//! `STARK_PARITY_NOW` pins deterministic screenshots to one instant so
//! relative labels (`20d`, `Today`) stay byte-stable across days. It
//! accepts Unix seconds, Unix milliseconds, or RFC 3339. Unset means the live
//! system clock. The value is read once per process.

use std::sync::OnceLock;

/// Wall-clock millis since the Unix epoch, optionally frozen by
/// `STARK_PARITY_NOW`.
pub fn now_millis() -> u64 {
    if let Some(ms) = frozen_millis() {
        return ms;
    }
    live_millis()
}

fn live_millis() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

fn frozen_millis() -> Option<u64> {
    static FROZEN: OnceLock<Option<u64>> = OnceLock::new();
    *FROZEN.get_or_init(|| {
        std::env::var("STARK_PARITY_NOW")
            .ok()
            .as_deref()
            .and_then(parse_stamp)
    })
}

/// Parse a frozen-clock stamp: digits are Unix seconds (13+ digits are
/// milliseconds); anything else is RFC 3339.
fn parse_stamp(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.chars().all(|c| c.is_ascii_digit()) {
        let n: u64 = raw.parse().ok()?;
        return Some(if raw.len() >= 13 {
            n
        } else {
            n.saturating_mul(1000)
        });
    }
    parse_rfc3339(raw)
}

/// Parse an RFC 3339 / ISO timestamp to Unix milliseconds. Public for
/// provider allowance reset times and other small wire values.
pub fn iso_millis(raw: &str) -> Option<u64> {
    parse_rfc3339(raw)
}

fn parse_rfc3339(raw: &str) -> Option<u64> {
    let split = raw.find(['T', 't', ' '])?;
    let (date, timed) = raw.split_at(split);
    let timed = &timed[1..];

    let mut date_parts = date.split('-');
    let y: i64 = date_parts.next()?.parse().ok()?;
    let mo: i64 = date_parts.next()?.parse().ok()?;
    let d: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }

    let (time, offset) =
        if let Some(t) = timed.strip_suffix('Z').or_else(|| timed.strip_suffix('z')) {
            (t, 0i64)
        } else if let Some(at) = timed.rfind('+').or_else(|| timed.rfind('-')) {
            let (t, zone) = timed.split_at(at);
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let zone = &zone[1..];
            let (zh, zm) = match zone.split_once(':') {
                Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
                None if zone.len() == 4 => (
                    zone.get(..2)?.parse::<i64>().ok()?,
                    zone.get(2..)?.parse::<i64>().ok()?,
                ),
                None => return None,
            };
            (t, sign * (zh * 3600 + zm * 60))
        } else {
            // A naive stamp is read as UTC, matching `Date.parse` in every
            // browser that observes ISO date-times without a zone.
            (timed, 0)
        };

    let (clock, frac) = match time.split_once('.') {
        Some((c, f)) => (c, Some(f)),
        None => (time, None),
    };
    let mut parts = clock.split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let mi: i64 = parts.next()?.parse().ok()?;
    let s: i64 = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() || h > 23 || mi > 59 || s > 60 {
        return None;
    }

    let millis = frac
        .and_then(|f| {
            let digits: String = f.chars().take(3).collect();
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let mut padded = digits;
            while padded.len() < 3 {
                padded.push('0');
            }
            padded.parse::<i64>().ok()
        })
        .unwrap_or(0);

    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset;
    u64::try_from(secs).ok().map(|s| s * 1000 + millis as u64)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Monotonic millis for run durations. Unlike [`now_millis`] this is never
/// frozen by the test clock, so a staged run still measures real elapsed
/// time in headless tests.
pub fn monotonic_millis() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unix_seconds_and_millis() {
        assert_eq!(parse_stamp("1704067200"), Some(1_704_067_200_000));
        assert_eq!(parse_stamp("1704067200250"), Some(1_704_067_200_250));
    }

    #[test]
    fn parses_rfc3339_zones_and_fractions() {
        let want = 1_704_067_200_250;
        assert_eq!(parse_stamp("2024-01-01T00:00:00.250Z"), Some(want));
        assert_eq!(parse_stamp("2024-01-01T02:00:00.250+02:00"), Some(want));
        assert_eq!(parse_stamp("2024-01-01T00:00:00.25z"), Some(want));
        assert_eq!(parse_stamp("2024-01-01 00:00:00.250"), Some(want));
    }

    #[test]
    fn rejects_bad_stamps() {
        for raw in [
            "",
            "x",
            "2024-13-01T00:00:00Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01",
        ] {
            assert_eq!(parse_stamp(raw), None, "{raw}");
        }
    }
}
