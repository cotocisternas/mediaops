//! Observed sizes and ages. No invented ETA or throughput.

pub fn fmt_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    let x = n as f64;
    if x >= TIB {
        fmt_scaled(x / TIB, "TiB")
    } else if x >= GIB {
        fmt_scaled(x / GIB, "GiB")
    } else if x >= MIB {
        format!("{:.0} MiB", x / MIB)
    } else if x >= KIB {
        format!("{:.0} KiB", x / KIB)
    } else {
        format!("{n} B")
    }
}

fn fmt_scaled(n: f64, unit: &str) -> String {
    if n >= 10.0 && (n - n.round()).abs() < 0.05 {
        format!("{:.0} {unit}", n.round())
    } else {
        format!("{n:.1} {unit}")
    }
}

pub fn fmt_age(secs: u64) -> String {
    if secs < 90 {
        format!("{secs}s")
    } else if secs < 90 * 60 {
        format!("{}m", secs / 60)
    } else if secs < 48 * 3600 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

pub fn fmt_percent(done: u64, total: u64) -> String {
    if total == 0 {
        return "n/a".into();
    }
    // Floor until all bytes arrive; a rounded 100% implies completion too early.
    format!("{}%", u128::from(done.min(total)) * 100 / u128::from(total))
}

pub fn fmt_progress(done: u64, total: u64) -> String {
    format!(
        "{} / {} ({})",
        fmt_bytes(done),
        fmt_bytes(total),
        fmt_percent(done, total)
    )
}

pub fn fmt_observed_age(timestamp: i64, now: i64) -> String {
    if timestamp <= 0 {
        "never".into()
    } else if timestamp > now {
        "clock ahead".into()
    } else {
        format!("{} ago", fmt_age((now - timestamp) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gib_and_seconds() {
        assert_eq!(fmt_bytes((71 * 1024 * 1024 * 1024) / 10), "7.1 GiB");
        assert_eq!(fmt_age(21 * 60), "21m");
        assert_eq!(fmt_age(2), "2s");
    }

    #[test]
    fn progress_handles_unknown_zero_and_large_sizes_without_false_completion() {
        assert_eq!(fmt_percent(0, 0), "n/a");
        assert_eq!(fmt_percent(999, 1000), "99%");
        assert_eq!(fmt_percent(u64::MAX - 1, u64::MAX), "99%");
        assert_eq!(fmt_percent(200, 100), "100%");
    }
}
