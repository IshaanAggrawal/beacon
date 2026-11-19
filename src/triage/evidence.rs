//! Everything behind a verdict, in a form that can be shown to a maintainer,
//! serialized for the web UI, or handed to an LLM to summarize.
//!
//! No opaque scores: every number comes with who said it, through which
//! projects, how long ago and why.

use std::collections::BTreeSet;
use std::time::Instant;

use serde::Serialize;

use super::verdict::{Thresholds, Tier, decide};
use crate::campaign::Cluster;
use crate::graph::TrustView;
use crate::graph::decay::age_days;
use crate::graph::trust::{Polarity, Statement, Support};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    pub subject: String,
    /// The project asking.
    pub root: String,
    pub tier: Tier,
    pub vouches: Side,
    pub denouncements: Side,
    pub campaign: Option<CampaignEvidence>,
    pub counterfactual: Counterfactual,
}

/// One side of the ledger: vouches or denouncements.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Side {
    /// Combined max-flow support; 1.0 is worth the root's own word.
    pub total: f64,
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub project: String,
    /// How much of `total` this source accounts for.
    pub weight: f64,
    /// Route from the root, root first, ending at `project`.
    pub path: Vec<String>,
    pub reason: Option<String>,
    /// None when the date wasn't recorded (some imported entries).
    pub age_days: Option<i64>,
    /// Below 1.0 when an old denouncement has faded.
    pub strength: f64,
    pub provisional: bool,
    pub under_appeal: bool,
    /// Maintainer who made the call, when known.
    pub actor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CampaignEvidence {
    pub prs: usize,
    pub accounts: usize,
    pub repos: usize,
    pub hours: i64,
    pub text_links: usize,
    pub layout_links: usize,
    /// Share of the cluster's PRs from accounts under a month old. Context
    /// only; account age never decides anything.
    pub young_share: f64,
}

impl From<&Cluster> for CampaignEvidence {
    fn from(c: &Cluster) -> Self {
        Self {
            prs: c.prs,
            accounts: c.authors,
            repos: c.repos,
            hours: (c.last_seen - c.first_seen).num_hours(),
            text_links: c.text_links,
            layout_links: c.layout_links,
            young_share: c.young_share,
        }
    }
}

/// The result of re-deciding with each contributing project removed in turn.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Counterfactual {
    /// How many projects were tried.
    pub checked: usize,
    /// Projects whose removal alone would change the verdict.
    pub flips: Vec<Flip>,
    pub micros: u128,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Flip {
    pub without: String,
    pub becomes: Tier,
}

impl Counterfactual {
    /// The verdict hangs on one project's word.
    pub fn single_source(&self) -> bool {
        !self.flips.is_empty()
    }
}

impl Side {
    fn from_support(view: &TrustView, s: &Support) -> Self {
        Self {
            total: s.total,
            sources: s.statements.iter().map(|st| source(view, st)).collect(),
        }
    }
}

fn source(view: &TrustView, st: &Statement) -> Source {
    let path = st
        .paths
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p.clone())
        .unwrap_or_else(|| vec![st.project.clone()]);
    Source {
        project: st.project.clone(),
        weight: st.amount,
        path,
        reason: st.mark.reason.clone(),
        age_days: st.mark.dated.then(|| age_days(st.mark.at, view.now)),
        strength: st.strength,
        provisional: st.provisional,
        under_appeal: st.under_appeal,
        actor: st.mark.actor.clone(),
    }
}

/// Decides on `subject` from `view.root`'s point of view and gathers the
/// evidence, including the single-source check.
pub fn assess(
    view: &TrustView,
    subject: &str,
    campaign: Option<&Cluster>,
    thresholds: &Thresholds,
) -> Evidence {
    let started = Instant::now();
    let in_campaign = campaign.is_some_and(|c| c.is_campaign);
    let vouches = view.support(subject, Polarity::For);
    let against = view.support(subject, Polarity::Against);
    let tier = decide(vouches.total, against.total, in_campaign, thresholds);

    // Every project the evidence flows through, except the root itself.
    let contributors: BTreeSet<&str> = vouches
        .statements
        .iter()
        .chain(&against.statements)
        .flat_map(|st| st.paths.iter().flat_map(|(p, _)| p.iter()))
        .map(String::as_str)
        .filter(|p| *p != view.root)
        .collect();

    let mut flips = Vec::new();
    for project in &contributors {
        let without = [*project];
        let v = view
            .support_excluding(subject, Polarity::For, &without)
            .total;
        let a = view
            .support_excluding(subject, Polarity::Against, &without)
            .total;
        let becomes = decide(v, a, in_campaign, thresholds);
        if becomes != tier {
            flips.push(Flip {
                without: project.to_string(),
                becomes,
            });
        }
    }

    Evidence {
        subject: subject.to_string(),
        root: view.root.clone(),
        tier,
        vouches: Side::from_support(view, &vouches),
        denouncements: Side::from_support(view, &against),
        campaign: campaign
            .filter(|c| c.is_campaign)
            .map(CampaignEvidence::from),
        counterfactual: Counterfactual {
            checked: contributors.len(),
            flips,
            micros: started.elapsed().as_micros(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Network, TrustEngine};
    use crate::ledger::attestation::tests::at;
    use crate::ledger::{Attestation, Kind, Settings};

    fn net(statements: &[(&str, Kind, &str, Option<f64>)]) -> Network {
        let mut by: std::collections::BTreeMap<&str, Vec<Attestation>> = Default::default();
        for (issuer, kind, subject, weight) in statements {
            let mut a = Attestation::new(*kind, issuer, subject, at(0));
            a.weight = *weight;
            if *kind == Kind::Denounce {
                a.reason = Some("same PR in 30 repos".into());
            }
            by.entry(issuer).or_default().push(a);
        }
        let mut n = Network::new();
        for (id, atts) in by {
            n.insert(id, &atts);
        }
        n
    }

    #[test]
    fn a_quarantine_resting_on_one_project_is_flagged_as_such() {
        let n = net(&[
            ("root", Kind::TrustPeer, "a", Some(0.9)),
            ("root", Kind::TrustPeer, "b", Some(0.3)),
            ("a", Kind::Denounce, "github:x", None),
            ("b", Kind::Denounce, "github:x", None),
        ]);
        let view = TrustEngine::new(&n, "root", Settings::default(), at(0)).run();
        let e = assess(&view, "github:x", None, &Thresholds::default());

        assert_eq!(e.tier, Tier::Quarantined);
        assert!(e.counterfactual.single_source());
        assert_eq!(
            e.counterfactual.flips,
            vec![
                Flip {
                    without: "a".into(),
                    becomes: Tier::CleanNewcomer
                },
                Flip {
                    without: "b".into(),
                    becomes: Tier::CleanNewcomer
                },
            ]
        );
        assert_eq!(
            e.denouncements.sources[0].reason.as_deref(),
            Some("same PR in 30 repos")
        );
    }

    #[test]
    fn corroborated_quarantine_survives_any_single_removal() {
        let n = net(&[
            ("root", Kind::TrustPeer, "a", Some(0.9)),
            ("root", Kind::TrustPeer, "b", Some(0.9)),
            ("root", Kind::TrustPeer, "c", Some(0.9)),
            ("a", Kind::Denounce, "github:x", None),
            ("b", Kind::Denounce, "github:x", None),
            ("c", Kind::Denounce, "github:x", None),
        ]);
        let view = TrustEngine::new(&n, "root", Settings::default(), at(0)).run();
        let e = assess(&view, "github:x", None, &Thresholds::default());
        assert_eq!(e.tier, Tier::Quarantined);
        assert_eq!(e.counterfactual.checked, 3);
        assert!(!e.counterfactual.single_source());
    }

    #[test]
    fn intermediate_projects_count_as_sources() {
        // root -> hub -> leaf; leaf vouches. Removing the hub cuts the path.
        let n = net(&[
            ("root", Kind::TrustPeer, "hub", Some(1.0)),
            ("hub", Kind::TrustPeer, "leaf", Some(1.0)),
            ("leaf", Kind::Vouch, "github:x", None),
        ]);
        let view = TrustEngine::new(&n, "root", Settings::default(), at(0)).run();
        let e = assess(&view, "github:x", None, &Thresholds::default());
        assert_eq!(e.tier, Tier::Trusted);
        let without: Vec<_> = e
            .counterfactual
            .flips
            .iter()
            .map(|f| f.without.as_str())
            .collect();
        assert_eq!(without, ["hub", "leaf"]);
        assert_eq!(e.vouches.sources[0].path, ["root", "hub", "leaf"]);
    }

    #[test]
    fn nobody_knows_them() {
        let n = net(&[("root", Kind::TrustPeer, "a", Some(0.9))]);
        let view = TrustEngine::new(&n, "root", Settings::default(), at(0)).run();
        let e = assess(&view, "github:new", None, &Thresholds::default());
        assert_eq!(e.tier, Tier::CleanNewcomer);
        assert_eq!(e.counterfactual.checked, 0);
        assert!(e.vouches.sources.is_empty() && e.denouncements.sources.is_empty());
    }
}
