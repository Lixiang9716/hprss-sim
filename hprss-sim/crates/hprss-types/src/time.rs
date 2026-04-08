//! Simulation time utilities with nanosecond precision.

/// Nanosecond timestamp. All simulation times are in nanoseconds.
pub type Nanos = u64;

/// Time constants for readability
pub const NS_PER_US: Nanos = 1_000;
pub const NS_PER_MS: Nanos = 1_000_000;
pub const NS_PER_SEC: Nanos = 1_000_000_000;

/// Convert microseconds to nanoseconds
#[inline]
pub const fn us(val: u64) -> Nanos {
    val * NS_PER_US
}

/// Convert milliseconds to nanoseconds
#[inline]
pub const fn ms(val: u64) -> Nanos {
    val * NS_PER_MS
}

/// Convert seconds to nanoseconds
#[inline]
pub const fn sec(val: u64) -> Nanos {
    val * NS_PER_SEC
}

/// Format nanoseconds as human-readable string
pub fn fmt_nanos(ns: Nanos) -> String {
    if ns >= NS_PER_SEC {
        format!("{:.3}s", ns as f64 / NS_PER_SEC as f64)
    } else if ns >= NS_PER_MS {
        format!("{:.3}ms", ns as f64 / NS_PER_MS as f64)
    } else if ns >= NS_PER_US {
        format!("{:.3}µs", ns as f64 / NS_PER_US as f64)
    } else {
        format!("{}ns", ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_conversions() {
        assert_eq!(us(1), 1_000);
        assert_eq!(ms(1), 1_000_000);
        assert_eq!(sec(1), 1_000_000_000);
    }

    #[test]
    fn fmt_display() {
        assert_eq!(fmt_nanos(500), "500ns");
        assert_eq!(fmt_nanos(1_500), "1.500µs");
        assert_eq!(fmt_nanos(2_500_000), "2.500ms");
        assert_eq!(fmt_nanos(1_200_000_000), "1.200s");
    }
}
