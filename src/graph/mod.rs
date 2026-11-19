//! The trust network as seen from one project.
//!
//! There is no global score. Every question is asked *from* a root project:
//! how much does it trust each other project, and how much independent,
//! trusted evidence is there for or against a given user?

pub mod blame;
pub mod decay;
pub mod flow;
pub mod trust;

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use crate::ledger::{Attestation, Kind, Origin};

pub use blame::{BadCall, Blame};
pub use trust::{ProjectTrust, Statement, Support, TrustEngine, TrustView};

/// One recorded decision by a project about a user.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub at: DateTime<Utc>,
    pub reason: Option<String>,
    pub scope: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Who made the call, when known: a maintainer name or "CODEOWNERS".
    pub actor: Option<String>,
    /// False for imported entries we couldn't date; `at` is then only a
    /// conservative guess and shouldn't be shown as a real date.
    pub dated: bool,
}

impl Mark {
    fn from_attestation(a: &Attestation) -> Self {
        let (actor, dated) = match &a.origin {
            // The importer only finds an actor when it also finds the commit.
            Origin::Imported { actor, .. } => (actor.clone(), actor.is_some()),
            Origin::Signed { .. } => (None, true),
        };
        Self {
            at: a.created_at,
            reason: a.reason.clone(),
            scope: a.scope.clone(),
            expires_at: a.expires_at,
            actor,
            dated,
        }
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|t| t > now)
    }
}

/// A project's current position on one user, after replaying its ledger.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserState {
    pub vouch: Option<Mark>,
    pub provisional: Option<Mark>,
    pub denounce: Option<Mark>,
    pub appeal: Option<Mark>,
}

/// A project's ledger, replayed into its current positions.
#[derive(Debug, Clone, Default)]
pub struct ProjectState {
    pub peers: BTreeMap<String, f64>,
    pub users: HashMap<String, UserState>,
}

impl ProjectState {
    pub fn from_attestations(attestations: &[Attestation]) -> Self {
        let mut ordered: Vec<&Attestation> = attestations.iter().collect();
        ordered.sort_by_key(|a| a.created_at); // stable: ledger order breaks ties

        let mut state = Self::default();
        for a in ordered {
            if a.kind == Kind::TrustPeer {
                match a.weight {
                    Some(w) if w > 0.0 => state.peers.insert(a.subject.clone(), w),
                    _ => state.peers.remove(&a.subject),
                };
                continue;
            }

            let user = state.users.entry(a.subject.clone()).or_default();
            let mark = Some(Mark::from_attestation(a));
            match a.kind {
                // The latest of vouch and denounce wins.
                Kind::Vouch => {
                    user.vouch = mark;
                    user.denounce = None;
                    user.appeal = None;
                }
                Kind::Denounce => {
                    user.denounce = mark;
                    user.vouch = None;
                    user.provisional = None;
                    user.appeal = None;
                }
                Kind::ProvisionalVouch => user.provisional = mark,
                Kind::Revoke => {
                    user.vouch = None;
                    user.provisional = None;
                }
                Kind::Appeal if user.denounce.is_some() => user.appeal = mark,
                Kind::Appeal => {}
                Kind::Rehabilitate => {
                    user.denounce = None;
                    user.appeal = None;
                }
                Kind::TrustPeer => unreachable!("handled above"),
            }
        }
        state
    }
}

/// Every project we know about, keyed by project id.
#[derive(Debug, Clone, Default)]
pub struct Network {
    pub projects: BTreeMap<String, ProjectState>,
}

impl Network {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a project. Only attestations issued by `id` are used;
    /// a project can't speak for anyone else.
    pub fn insert(&mut self, id: &str, attestations: &[Attestation]) {
        let own: Vec<Attestation> = attestations
            .iter()
            .filter(|a| a.issuer == id)
            .cloned()
            .collect();
        self.projects
            .insert(id.to_string(), ProjectState::from_attestations(&own));
    }

    pub fn remove(&mut self, id: &str) {
        self.projects.remove(id);
    }

    pub fn user(&self, project: &str, subject: &str) -> Option<&UserState> {
        self.projects.get(project)?.users.get(subject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::attestation::tests::at;

    fn att(kind: Kind, subject: &str, day: u32) -> Attestation {
        Attestation::new(kind, "p", subject, at(day))
    }

    #[test]
    fn replays_in_time_order() {
        let state = ProjectState::from_attestations(&[
            att(Kind::Denounce, "github:a", 5),
            att(Kind::Vouch, "github:a", 1),
        ]);
        let a = &state.users["github:a"];
        assert!(a.denounce.is_some() && a.vouch.is_none());
    }

    #[test]
    fn rehabilitation_clears_a_denouncement() {
        let state = ProjectState::from_attestations(&[
            att(Kind::Denounce, "github:a", 1),
            att(Kind::Appeal, "github:a", 2),
            att(Kind::Rehabilitate, "github:a", 3),
        ]);
        assert_eq!(state.users["github:a"], UserState::default());
    }

    #[test]
    fn revoke_withdraws_a_vouch() {
        let state = ProjectState::from_attestations(&[
            att(Kind::Vouch, "github:a", 1),
            att(Kind::Revoke, "github:a", 2),
        ]);
        assert!(state.users["github:a"].vouch.is_none());
    }

    #[test]
    fn zero_weight_drops_a_peer() {
        let mut up = Attestation::new(Kind::TrustPeer, "p", "zig", at(1));
        up.weight = Some(0.8);
        let mut down = up.clone();
        down.created_at = at(2);
        down.weight = Some(0.0);
        assert!(
            ProjectState::from_attestations(&[up, down])
                .peers
                .is_empty()
        );
    }

    #[test]
    fn a_project_cannot_speak_for_another() {
        let mut net = Network::new();
        let forged = Attestation::new(Kind::Denounce, "ghostty", "github:a", at(1));
        net.insert("hostile", &[forged]);
        assert!(net.user("hostile", "github:a").is_none());
    }
}
