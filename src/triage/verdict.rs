//! The three answers beacon gives about a pull request's author.
//!
//! Identity (who vouches for them) and behavior (are they part of a
//! campaign) are judged separately, then combined:
//!
//! - **Quarantined**: corroborated denouncements, or caught in a campaign
//!   without strong vouches to outweigh it.
//! - **Trusted**: enough vouches from projects you trust.
//! - **Clean newcomer**: everyone else. Unknown is not suspicious; these are
//!   never auto-blocked.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Trusted,
    CleanNewcomer,
    Quarantined,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Trusted => "Trusted",
            Tier::CleanNewcomer => "Clean newcomer",
            Tier::Quarantined => "Quarantined",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Vouch support needed to fast-track someone.
    pub trusted: f64,
    /// Denouncement support needed to quarantine: 1.0 means your own word,
    /// or several independent projects you trust adding up to it.
    pub quarantine: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            trusted: 0.5,
            quarantine: 1.0,
        }
    }
}

/// Inputs are max-flow support totals from the trust graph (`for` and
/// `against`) and whether the PR matched an active campaign.
pub fn decide(vouches: f64, denouncements: f64, in_campaign: bool, t: &Thresholds) -> Tier {
    const EPS: f64 = 1e-9;
    if denouncements + EPS >= t.quarantine {
        Tier::Quarantined
    } else if in_campaign && vouches + EPS < t.quarantine {
        // Only a vouch as strong as your own word outweighs a campaign match.
        Tier::Quarantined
    } else if vouches + EPS >= t.trusted {
        Tier::Trusted
    } else {
        Tier::CleanNewcomer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(v: f64, a: f64, c: bool) -> Tier {
        decide(v, a, c, &Thresholds::default())
    }

    #[test]
    fn unknown_people_are_clean_newcomers() {
        assert_eq!(d(0.0, 0.0, false), Tier::CleanNewcomer);
    }

    #[test]
    fn one_weak_denouncement_does_not_quarantine() {
        assert_eq!(d(0.0, 0.6, false), Tier::CleanNewcomer);
        assert_eq!(d(0.0, 0.6 + 0.4, false), Tier::Quarantined);
    }

    #[test]
    fn campaigns_beat_moderate_vouches_but_not_your_own() {
        assert_eq!(d(0.7, 0.0, true), Tier::Quarantined);
        assert_eq!(d(1.0, 0.0, true), Tier::Trusted);
    }

    #[test]
    fn vouches_fast_track() {
        assert_eq!(d(0.5, 0.0, false), Tier::Trusted);
        assert_eq!(d(0.49, 0.0, false), Tier::CleanNewcomer);
    }
}
