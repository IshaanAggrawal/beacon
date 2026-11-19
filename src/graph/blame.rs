//! Backward blame: vouching carries reputational stake.
//!
//! From the root's point of view, a peer makes a *bad call* when it
//!
//! - vouches for someone the root has quarantined (denounced, or caught in a
//!   campaign), or
//! - denounces someone the root itself vouches for.
//!
//! Each bad call multiplies the root's trust in that peer by `blame_factor`
//! (0.7 by default), and fades with `blame_half_life_days`. One mistake dents
//! a peer; a pattern of them cuts it off. Nobody has to notice and edit a
//! config file for that to happen.
//!
//! Counting bad calls, rather than the share of a peer's vouches that went
//! bad, is deliberate: a ratio could be diluted by mass-vouching for
//! harmless accounts.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::decay;
use super::{Network, ProjectState};
use crate::ledger::Settings;

#[derive(Debug, Clone, PartialEq)]
pub enum BadCall {
    /// The peer vouched for someone the root has quarantined.
    VouchedForQuarantined { subject: String },
    /// The peer denounced someone the root vouches for.
    DenouncedRootsVouchee { subject: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Blame {
    pub calls: Vec<(BadCall, f64)>,
    /// What the root's trust in this peer gets multiplied by.
    pub multiplier: f64,
}

/// `quarantined` maps each user the root considers bad to when that became known.
pub fn compute(
    net: &Network,
    root: &str,
    quarantined: &BTreeMap<String, DateTime<Utc>>,
    settings: &Settings,
    now: DateTime<Utc>,
) -> BTreeMap<String, Blame> {
    let empty = ProjectState::default();
    let root_state = net.projects.get(root).unwrap_or(&empty);

    let mut out = BTreeMap::new();
    for (id, state) in &net.projects {
        if id == root {
            continue;
        }
        let mut calls = Vec::new();
        for (subject, user) in &state.users {
            if let (Some(vouch), Some(&known)) = (&user.vouch, quarantined.get(subject)) {
                // Blame starts fading from whichever came later.
                let since = known.max(vouch.at);
                calls.push((
                    BadCall::VouchedForQuarantined {
                        subject: subject.clone(),
                    },
                    decay::weight(since, now, settings.blame_half_life_days),
                ));
            }
            let root_vouches = root_state
                .users
                .get(subject)
                .is_some_and(|u| u.vouch.is_some());
            if let (Some(denounce), true) = (&user.denounce, root_vouches) {
                calls.push((
                    BadCall::DenouncedRootsVouchee {
                        subject: subject.clone(),
                    },
                    decay::weight(denounce.at, now, settings.blame_half_life_days),
                ));
            }
        }
        if calls.is_empty() {
            continue;
        }
        calls.sort_by(|a, b| b.1.total_cmp(&a.1));
        let penalty: f64 = calls.iter().map(|(_, w)| w).sum();
        out.insert(
            id.clone(),
            Blame {
                multiplier: settings.blame_factor.powf(penalty),
                calls,
            },
        );
    }
    out
}
