use std::time::{SystemTime, UNIX_EPOCH};

/// Human readable binary size, e.g. `1.4 GB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Relative age from a Unix timestamp, e.g. `3d ago`.
pub fn format_age(modified: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(modified);
    match delta {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m ago", delta / 60),
        3600..=86_399 => format!("{}h ago", delta / 3600),
        86_400..=2_591_999 => format!("{}d ago", delta / 86_400),
        2_592_000..=31_535_999 => format!("{}mo ago", delta / 2_592_000),
        _ => format!("{}y ago", delta / 31_536_000),
    }
}

pub fn format_elapsed(ms: u128) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}.{:01}s", (ms % 1000) / 100)
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Keep paths readable in narrow panes by dropping the front.
pub fn ellipsize(text: &str, max: usize) -> String {
    let len = text.chars().count();
    if len <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let tail: String = text.chars().skip(len - (max - 1)).collect();
    format!("…{tail}")
}

/// Keep the beginning of a line readable (for status messages).
pub fn truncate_right(text: &str, max: usize) -> String {
    let len = text.chars().count();
    if len <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let head: String = text.chars().take(max - 1).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn ellipsis_counts_chars_not_bytes() {
        assert_eq!(ellipsize("abc", 10), "abc");
        assert_eq!(ellipsize("abcdefgh", 5), "…efgh");
        assert_eq!(ellipsize("日本語のパス", 4), "…のパス");
    }

    #[test]
    fn truncate_keeps_head() {
        assert_eq!(truncate_right("abcdefgh", 5), "abcd…");
        assert_eq!(truncate_right("abc", 5), "abc");
    }
}
