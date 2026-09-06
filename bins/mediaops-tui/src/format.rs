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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gib_and_seconds() {
        assert_eq!(fmt_bytes((71 * 1024 * 1024 * 1024) / 10), "7.1 GiB");
        assert_eq!(fmt_age(21 * 60), "21m");
        assert_eq!(fmt_age(2), "2s");
    }
}
