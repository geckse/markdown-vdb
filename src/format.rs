use std::time::SystemTime;

/// Format a timestamp as a human-readable relative time string.
///
/// Uses `SystemTime` directly to avoid a `chrono` dependency.
pub fn format_timestamp(time: SystemTime) -> String {
    let elapsed = match SystemTime::now().duration_since(time) {
        Ok(d) => d,
        Err(_) => return "in the future".to_string(),
    };

    let secs = elapsed.as_secs();
    if secs < 60 {
        return "just now".to_string();
    }

    let mins = secs / 60;
    if mins < 60 {
        return if mins == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{mins} minutes ago")
        };
    }

    let hours = mins / 60;
    if hours < 24 {
        return if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{hours} hours ago")
        };
    }

    let days = hours / 24;
    if days == 1 {
        "1 day ago".to_string()
    } else {
        format!("{days} days ago")
    }
}

/// Format a byte count as a human-readable file size (1024-based).
pub fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Render an ASCII progress/percentage bar.
///
/// `filled` is the number of filled segments, `total` is the bar width.
/// Uses `█` for filled and `░` for unfilled segments.
pub fn render_bar(filled: usize, total: usize) -> String {
    let filled = filled.min(total);
    let unfilled = total - filled;
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(unfilled),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamp_just_now() {
        let time = SystemTime::now() - Duration::from_secs(30);
        assert_eq!(format_timestamp(time), "just now");
    }

    #[test]
    fn timestamp_one_minute() {
        let time = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(format_timestamp(time), "1 minute ago");
    }

    #[test]
    fn timestamp_multiple_minutes() {
        let time = SystemTime::now() - Duration::from_secs(300);
        assert_eq!(format_timestamp(time), "5 minutes ago");
    }

    #[test]
    fn timestamp_one_hour() {
        let time = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(format_timestamp(time), "1 hour ago");
    }

    #[test]
    fn timestamp_multiple_hours() {
        let time = SystemTime::now() - Duration::from_secs(7200);
        assert_eq!(format_timestamp(time), "2 hours ago");
    }

    #[test]
    fn timestamp_one_day() {
        let time = SystemTime::now() - Duration::from_secs(86400);
        assert_eq!(format_timestamp(time), "1 day ago");
    }

    #[test]
    fn timestamp_multiple_days() {
        let time = SystemTime::now() - Duration::from_secs(86400 * 7);
        assert_eq!(format_timestamp(time), "7 days ago");
    }

    #[test]
    fn timestamp_future() {
        let time = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(format_timestamp(time), "in the future");
    }

    #[test]
    fn file_size_bytes() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(1023), "1023 B");
    }

    #[test]
    fn file_size_kilobytes() {
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
    }

    #[test]
    fn file_size_megabytes() {
        assert_eq!(format_file_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 5), "5.0 MB");
    }

    #[test]
    fn file_size_gigabytes() {
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn bar_full() {
        assert_eq!(render_bar(10, 10), "██████████");
    }

    #[test]
    fn bar_empty() {
        assert_eq!(render_bar(0, 10), "░░░░░░░░░░");
    }

    #[test]
    fn bar_half() {
        assert_eq!(render_bar(5, 10), "█████░░░░░");
    }

    #[test]
    fn bar_overflow_clamped() {
        assert_eq!(render_bar(15, 10), "██████████");
    }

    #[test]
    fn bar_zero_width() {
        assert_eq!(render_bar(0, 0), "");
    }
}
