use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Formats a duration as `m:ss`, or `h:mm:ss` once it reaches an hour.
pub fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total / 60) % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Formats a long total such as an album or playlist length: "1 hr 4 min", "38 min".
/// Under half a minute, which would read "0 min", it is `m:ss` instead.
pub fn format_total_duration(duration: Duration) -> String {
    if duration < Duration::from_secs(30) {
        return format_duration(duration);
    }
    let minutes = (duration.as_secs() + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} hr"),
        (h, m) => format!("{h} hr {m} min"),
    }
}

/// Formats a count with thousands separators: `12345` → `"12,345"`.
pub fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Seconds since the Unix epoch, used for timestamps stored in the database.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// 64-bit FNV-1a hash. Stable across builds and platforms, unlike `std`'s hasher,
/// so it is safe to use for on-disk cache keys.
pub fn stable_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0:00");
        assert_eq!(format_duration(Duration::from_secs(225)), "3:45");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(
            format_total_duration(Duration::from_secs(38 * 60)),
            "38 min"
        );
        assert_eq!(
            format_total_duration(Duration::from_secs(3840)),
            "1 hr 4 min"
        );
        assert_eq!(format_total_duration(Duration::from_secs(7200)), "2 hr");
        assert_eq!(format_total_duration(Duration::from_secs(24)), "0:24");
        assert_eq!(format_total_duration(Duration::from_secs(30)), "1 min");
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(stable_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
