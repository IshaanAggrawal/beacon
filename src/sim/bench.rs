//! Replays a simulated PR stream through flat Vouch and through beacon, in
//! time order, and scores every decision against the ground truth.
//!
//! Each PR is judged by the project receiving it, using only what was known
//! when it arrived. Early PRs of a campaign can slip through before the
//! campaign is visible; that's real and it's counted.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

use chrono::{DateTime, Utc};

use super::world::{Truth, World};
use crate::campaign::{Cluster, Detector, DetectorConfig, Fingerprint, Salt};
use crate::graph::{Network, TrustEngine, TrustView};
use crate::ledger::Settings;
use crate::triage::{Evidence, Thresholds, Tier, assess};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum System {
    /// Only people the receiving project vouched for get in.
    FlatRequireVouch,
    /// Everyone gets in unless the receiving project denounced them.
    FlatDenounceOnly,
    /// Also adopt the denounce lists of every project you trust.
    FlatSlurp,
    Beacon,
}

impl System {
    pub const ALL: [System; 4] = [
        System::FlatRequireVouch,
        System::FlatDenounceOnly,
        System::FlatSlurp,
        System::Beacon,
    ];

    pub fn name(self) -> &'static str {
        match self {
            System::FlatRequireVouch => "Flat Vouch, require vouch",
            System::FlatDenounceOnly => "Flat Vouch, denounce only",
            System::FlatSlurp => "Flat Vouch + peers' denounce lists",
            System::Beacon => "beacon",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Score {
    pub prs: BTreeMap<Truth, usize>,
    pub blocked: BTreeMap<Truth, usize>,
    /// Distinct honest people with at least one PR blocked.
    pub honest_people_blocked: BTreeSet<String>,
    /// Of those, people the hostile project and Sybil ring were smearing.
    pub smeared_people_blocked: BTreeSet<String>,
    /// Hours from each campaign's first PR to its first blocked PR.
    pub hours_to_first_block: BTreeMap<u8, Option<f64>>,
    /// PRs let through but shown to a maintainer with a warning: some
    /// denouncement support, short of quarantine. Only beacon does this.
    pub flagged: BTreeMap<Truth, usize>,
}

impl Score {
    fn rate(&self, t: Truth) -> f64 {
        let n = self.prs.get(&t).copied().unwrap_or(0);
        if n == 0 {
            0.0
        } else {
            self.blocked.get(&t).copied().unwrap_or(0) as f64 / n as f64
        }
    }

    pub fn flagged_rate(&self, t: Truth) -> f64 {
        let n = self.prs.get(&t).copied().unwrap_or(0);
        if n == 0 {
            0.0
        } else {
            self.flagged.get(&t).copied().unwrap_or(0) as f64 / n as f64
        }
    }

    pub fn spam_caught(&self) -> f64 {
        self.rate(Truth::Spam)
    }

    pub fn offenders_caught(&self) -> f64 {
        self.rate(Truth::RepeatOffender)
    }

    pub fn newcomers_blocked(&self) -> f64 {
        self.rate(Truth::Newcomer)
    }

    pub fn regulars_blocked(&self) -> f64 {
        self.rate(Truth::Regular)
    }

    /// Share of blocked PRs that really were bad.
    pub fn precision(&self) -> f64 {
        let total: usize = self.blocked.values().sum();
        let bad: usize = self
            .blocked
            .iter()
            .filter(|(t, _)| t.is_bad())
            .map(|(_, n)| n)
            .sum();
        if total == 0 {
            1.0
        } else {
            bad as f64 / total as f64
        }
    }
}

/// An honest PR that beacon quarantined, and the numbers behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct FalseBlock {
    pub author: String,
    pub target: String,
    pub truth: Truth,
    pub vouches: f64,
    pub denouncements: f64,
    pub denounced_by: Vec<(String, f64)>,
    pub in_campaign: bool,
    pub cluster: Option<Cluster>,
}

/// Everything a run produced, for tables and for the demo.
#[derive(Debug, Clone)]
pub struct Run {
    pub scores: BTreeMap<System, Score>,
    pub false_blocks: Vec<FalseBlock>,
    /// beacon's verdict and evidence for every PR, in stream order.
    pub evidence: Vec<Evidence>,
    pub campaign_flags: BTreeMap<String, DateTime<Utc>>,
}

pub fn run(world: &World) -> Run {
    let net = world.network();
    let settings = Settings::default();
    let thresholds = Thresholds::default();
    let mut detector = Detector::new(DetectorConfig::default());
    let salt = Salt([0x5a; 32]);

    let mut scores: BTreeMap<System, Score> =
        System::ALL.iter().map(|s| (*s, Score::default())).collect();
    let mut first_pr: BTreeMap<u8, DateTime<Utc>> = BTreeMap::new();
    let mut flagged: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
    let mut author_cluster: HashMap<String, usize> = HashMap::new();
    let mut evidence = Vec::with_capacity(world.prs.len());
    let mut false_blocks = Vec::new();
    // Trust views only change when the set of flagged accounts does.
    let mut views: HashMap<String, (usize, TrustView)> = HashMap::new();

    for pr in &world.prs {
        let author = &pr.event.author;
        if let Some(c) = pr.campaign {
            first_pr.entry(c).or_insert(pr.event.at);
        }

        let (idx, cluster) = detector.observe(Fingerprint::of(&pr.event, &salt));
        if cluster.is_campaign {
            flagged.entry(author.clone()).or_insert(pr.event.at);
            author_cluster.entry(author.clone()).or_insert(idx);
        }
        // Once caught in a campaign, an account stays flagged.
        let campaign = author_cluster.get(author).map(|i| detector.cluster_of(*i));

        let stale = views
            .get(&pr.target)
            .is_none_or(|(v, _)| *v != flagged.len());
        if stale {
            let mut engine = TrustEngine::new(&net, &pr.target, settings.clone(), pr.event.at);
            for (who, at) in &flagged {
                engine = engine.flag(who, *at);
            }
            views.insert(pr.target.clone(), (flagged.len(), engine.run()));
        }
        let view = &views[&pr.target].1;
        let ev = assess(view, author, campaign.as_ref(), &thresholds);
        let tier = ev.tier;
        let against = ev.denouncements.total;
        if tier == Tier::Quarantined && !pr.truth.is_bad() {
            false_blocks.push(FalseBlock {
                author: author.clone(),
                target: pr.target.clone(),
                truth: pr.truth,
                vouches: ev.vouches.total,
                denouncements: against,
                denounced_by: ev
                    .denouncements
                    .sources
                    .iter()
                    .map(|s| (s.project.clone(), s.weight))
                    .collect(),
                in_campaign: campaign.is_some(),
                cluster: campaign.clone(),
            });
        }
        evidence.push(ev);

        for system in System::ALL {
            let blocked = match system {
                System::FlatRequireVouch => !flat_vouched(&net, &pr.target, author),
                System::FlatDenounceOnly => flat_denounced(&net, &pr.target, author),
                System::FlatSlurp => {
                    flat_denounced(&net, &pr.target, author)
                        || net.projects.get(&pr.target).is_some_and(|p| {
                            p.peers
                                .keys()
                                .any(|peer| flat_denounced(&net, peer, author))
                        })
                }
                System::Beacon => tier == Tier::Quarantined,
            };
            let score = scores.get_mut(&system).expect("all systems scored");
            *score.prs.entry(pr.truth).or_default() += 1;
            if let Some(c) = pr.campaign {
                score.hours_to_first_block.entry(c).or_insert(None);
            }
            if !blocked {
                if system == System::Beacon && against > 1e-9 {
                    *score.flagged.entry(pr.truth).or_default() += 1;
                }
                continue;
            }
            *score.blocked.entry(pr.truth).or_default() += 1;
            if !pr.truth.is_bad() {
                score.honest_people_blocked.insert(author.clone());
                if world.smeared.contains(author) {
                    score.smeared_people_blocked.insert(author.clone());
                }
            }
            if let Some(c) = pr.campaign {
                let entry = score.hours_to_first_block.entry(c).or_insert(None);
                if entry.is_none() {
                    let hours = (pr.event.at - first_pr[&c]).num_minutes() as f64 / 60.0;
                    *entry = Some(hours);
                }
            }
        }
    }

    Run {
        scores,
        false_blocks,
        evidence,
        campaign_flags: flagged,
    }
}

fn flat_vouched(net: &Network, project: &str, who: &str) -> bool {
    net.user(project, who).is_some_and(|u| u.vouch.is_some())
}

fn flat_denounced(net: &Network, project: &str, who: &str) -> bool {
    net.user(project, who).is_some_and(|u| u.denounce.is_some())
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// A few PRs worth showing someone: one of each interesting situation.
pub fn samples(world: &World, run: &Run) -> Vec<(&'static str, usize)> {
    let find = |pred: &dyn Fn(usize) -> bool| (0..world.prs.len()).find(|i| pred(*i));
    let pr = |i: usize| &world.prs[i];
    let ev = |i: usize| &run.evidence[i];
    let picks: [(&'static str, Option<usize>); 6] = [
        (
            "A reworded spam PR, caught as part of a campaign",
            find(&|i| pr(i).campaign == Some(1) && ev(i).tier == Tier::Quarantined),
        ),
        (
            "A repeat offender quarantined on denouncements",
            find(&|i| pr(i).truth == Truth::RepeatOffender && ev(i).tier == Tier::Quarantined),
        ),
        (
            "A repeat offender, let through with a warning",
            find(&|i| {
                pr(i).truth == Truth::RepeatOffender
                    && ev(i).tier != Tier::Quarantined
                    && !ev(i).denouncements.sources.is_empty()
            }),
        ),
        (
            "Someone the hostile project and Sybil ring are smearing",
            find(&|i| {
                world.smeared.contains(&pr(i).event.author) && pr(i).target == super::world::GHOSTTY
            }),
        ),
        (
            "A trusted contributor, vouched for two hops away",
            find(&|i| {
                ev(i).tier == Tier::Trusted
                    && ev(i).vouches.sources.iter().all(|s| s.path.len() > 2)
            }),
        ),
        (
            "An honest newcomer nobody knows yet",
            find(&|i| {
                pr(i).truth == Truth::Newcomer
                    && ev(i).vouches.sources.is_empty()
                    && ev(i).denouncements.sources.is_empty()
            }),
        ),
    ];
    picks
        .into_iter()
        .filter_map(|(t, i)| i.map(|i| (t, i)))
        .collect()
}

/// Plain-English list of the honest PRs beacon blocked, and why.
pub fn false_blocks(run: &Run) -> String {
    let mut out = String::new();
    if run.false_blocks.is_empty() {
        let _ = writeln!(out, "beacon didn't block any honest PRs.");
        return out;
    }
    let _ = writeln!(out, "Honest PRs beacon blocked, and why:");
    for f in &run.false_blocks {
        let why = if f.denouncements >= 1.0 - 1e-9 {
            let who: Vec<String> = f
                .denounced_by
                .iter()
                .map(|(p, a)| format!("{p} ({})", pct(*a)))
                .collect();
            format!("denounced by {}", who.join(" + "))
        } else if let Some(c) = &f.cluster {
            format!(
                "matched a campaign of {} PRs from {} accounts in {} repos ({} text links, {} layout links); vouches only reached {}",
                c.prs,
                c.authors,
                c.repos,
                c.text_links,
                c.layout_links,
                pct(f.vouches)
            )
        } else {
            format!(
                "matched a campaign; vouches only reached {}",
                pct(f.vouches)
            )
        };
        let _ = writeln!(
            out,
            "  - {} ({:?}) at {}: {why}",
            f.author, f.truth, f.target
        );
    }
    out
}

/// A Markdown table, one row per system.
pub fn table(world: &World, run: &Run) -> String {
    let mut out = String::new();
    let any = &run.scores[&System::Beacon];
    let n = |t: Truth| any.prs.get(&t).copied().unwrap_or(0);
    let _ = writeln!(
        out,
        "Seed {}: {} PRs over {} days ({} regular, {} from newcomers, {} campaign spam, {} from repeat offenders).\n",
        world.config.seed,
        world.prs.len(),
        world.config.days,
        n(Truth::Regular),
        n(Truth::Newcomer),
        n(Truth::Spam),
        n(Truth::RepeatOffender),
    );
    let _ = writeln!(
        out,
        "| System | Campaign spam caught | Repeat offenders caught | Newcomer PRs blocked | Regular PRs blocked | Honest people blocked | Smear victims blocked | Precision | Hours to first block (copy-paste / paraphrased) |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---|");
    for (system, s) in &run.scores {
        let hours = |c: u8| match s.hours_to_first_block.get(&c).copied().flatten() {
            Some(h) => format!("{h:.1}"),
            None => "never".into(),
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} / {} | {} | {} / {} |",
            system.name(),
            pct(s.spam_caught()),
            pct(s.offenders_caught()),
            pct(s.newcomers_blocked()),
            pct(s.regulars_blocked()),
            s.honest_people_blocked.len(),
            s.smeared_people_blocked.len(),
            world.smeared.len(),
            if s.blocked.is_empty() {
                "—".to_string()
            } else {
                pct(s.precision())
            },
            hours(0),
            hours(1),
        );
    }
    let b = &run.scores[&System::Beacon];
    let _ = writeln!(
        out,
        "
beacon also let through, with a \"review first\" warning: {} of repeat-offender PRs, {} of newcomer PRs and {} of regular PRs.",
        pct(b.flagged_rate(Truth::RepeatOffender)),
        pct(b.flagged_rate(Truth::Newcomer)),
        pct(b.flagged_rate(Truth::Regular)),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::world::WorldConfig;

    #[test]
    fn beacon_beats_every_flat_baseline_where_it_matters() {
        let world = World::generate(WorldConfig::default());
        let run = run(&world);
        let beacon = &run.scores[&System::Beacon];

        // Never auto-blocks honest newcomers, unlike require-vouch.
        assert_eq!(beacon.newcomers_blocked(), 0.0, "{}", table(&world, &run));
        assert!(run.scores[&System::FlatRequireVouch].newcomers_blocked() > 0.9);

        // Catches the campaigns the flat lists can't see.
        for flat in [System::FlatDenounceOnly, System::FlatSlurp] {
            assert!(beacon.spam_caught() > run.scores[&flat].spam_caught() + 0.5);
        }

        // Smears from the hostile project and Sybil ring don't land.
        assert_eq!(
            beacon.smeared_people_blocked.len(),
            0,
            "{}",
            table(&world, &run)
        );
        assert!(
            !run.scores[&System::FlatSlurp]
                .smeared_people_blocked
                .is_empty()
        );
    }
}
