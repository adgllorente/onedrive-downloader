pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

pub fn human_eta(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_use_decimal_units_and_comma() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1_500_000), "1,5 MB");
        assert_eq!(human_bytes(14_250_280_807), "14,3 GB");
    }

    #[test]
    fn eta_is_readable() {
        assert_eq!(human_eta(45), "45 s");
        assert_eq!(human_eta(600), "10 min");
        assert_eq!(human_eta(7260), "2 h 1 min");
    }
}
