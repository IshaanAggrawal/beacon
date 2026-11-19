//! Exponential time decay, so old judgments fade instead of lasting forever.

use chrono::{DateTime, Utc};

/// Weight of something that happened at `then`, as seen at `now`: 1.0 when
/// fresh, 0.5 after one half-life, 0.25 after two. Future dates count as fresh.
pub fn weight(then: DateTime<Utc>, now: DateTime<Utc>, half_life_days: f64) -> f64 {
    let age_days = (now - then).num_seconds() as f64 / 86_400.0;
    if age_days <= 0.0 || half_life_days <= 0.0 {
        return 1.0;
    }
    0.5_f64.powf(age_days / half_life_days)
}

pub fn age_days(then: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    (now - then).num_days().max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::attestation::tests::at;

    #[test]
    fn halves_every_half_life() {
        assert_eq!(weight(at(0), at(0), 180.0), 1.0);
        assert!((weight(at(0), at(180), 180.0) - 0.5).abs() < 1e-9);
        assert!((weight(at(0), at(360), 180.0) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn future_dates_and_disabled_decay_count_fully() {
        assert_eq!(weight(at(10), at(0), 180.0), 1.0);
        assert_eq!(weight(at(0), at(1000), 0.0), 1.0);
    }
}
