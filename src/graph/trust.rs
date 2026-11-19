//! Subjective trust from one root project.
//!
//! Two questions, answered separately:
//!
//! 1. **How much does the root trust each project?** The best path wins:
//!    multiply edge weights along it, apply `hop_decay` for every hop past a
//!    direct peer, and apply backward blame to each project on the way.
//!
//! 2. **How much trusted, independent evidence is there for or against a
//!    user?** This is a max-flow from the root to the user, where each
//!    project can pass on at most as much as the root trusts it (the
//!    Advogato idea). Ten fake projects hiding behind one real one can't
//!    add up to more than that one real project is worth, and the flow's
//!    paths tell us exactly whose word the answer rests on.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use super::blame::{self, Blame};
use super::decay;
use super::flow::{FlowGraph, UNBOUNDED};
use super::{Mark, Network};
use crate::ledger::Settings;

const MIN_TRUST: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTrust {
    pub trust: f64,
    pub hops: usize,
    /// Best path from the root, root first.
    pub path: Vec<String>,
    pub blame: Option<Blame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    /// Vouches.
    For,
    /// Denouncements.
    Against,
}

/// One project's statement about a user and how much of it reached the root.
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub project: String,
    /// Support credited to this statement after trust caps.
    pub amount: f64,
    /// 1.0, or less if the statement has faded with age.
    pub strength: f64,
    pub mark: Mark,
    pub provisional: bool,
    pub under_appeal: bool,
    /// Routes the support took from the root, with how much went each way.
    pub paths: Vec<(Vec<String>, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Support {
    pub polarity: Polarity,
    pub total: f64,
    /// Largest contribution first.
    pub statements: Vec<Statement>,
}

impl Support {
    pub fn is_empty(&self) -> bool {
        self.total <= MIN_TRUST
    }
}

pub struct TrustEngine<'a> {
    net: &'a Network,
    root: String,
    settings: Settings,
    now: DateTime<Utc>,
    flagged: BTreeMap<String, DateTime<Utc>>,
}

impl<'a> TrustEngine<'a> {
    pub fn new(net: &'a Network, root: &str, settings: Settings, now: DateTime<Utc>) -> Self {
        Self {
            net,
            root: root.to_string(),
            settings,
            now,
            flagged: BTreeMap::new(),
        }
    }

    /// Marks a user as quarantined for blame purposes, e.g. because they were
    /// caught in a campaign. The root's own denouncements count automatically.
    pub fn flag(mut self, subject: &str, since: DateTime<Utc>) -> Self {
        self.flagged.insert(subject.to_string(), since);
        self
    }

    pub fn run(self) -> TrustView<'a> {
        let mut quarantined = self.flagged.clone();
        if let Some(root) = self.net.projects.get(&self.root) {
            for (subject, user) in &root.users {
                if let Some(d) = &user.denounce {
                    quarantined.entry(subject.clone()).or_insert(d.at);
                }
            }
        }
        let blames = blame::compute(self.net, &self.root, &quarantined, &self.settings, self.now);
        let multiplier = |id: &str, blames: &BTreeMap<String, Blame>| {
            blames.get(id).map_or(1.0, |b| b.multiplier)
        };

        let mut best: BTreeMap<String, ProjectTrust> = BTreeMap::new();
        best.insert(
            self.root.clone(),
            ProjectTrust {
                trust: 1.0,
                hops: 0,
                path: vec![self.root.clone()],
                blame: None,
            },
        );

        // Relax one hop at a time so `max_hops` is respected exactly.
        let mut frontier = vec![self.root.clone()];
        for hop in 1..=self.settings.max_hops {
            let mut next: BTreeMap<&str, (f64, &str)> = BTreeMap::new();
            for p in &frontier {
                let Some(state) = self.net.projects.get(p) else {
                    continue;
                };
                let hop_factor = if *p == self.root {
                    1.0
                } else {
                    self.settings.hop_decay
                };
                for (q, w) in &state.peers {
                    if *q == self.root {
                        continue;
                    }
                    let cand = best[p].trust * w * hop_factor * multiplier(q, &blames);
                    if cand > next.get(q.as_str()).map_or(MIN_TRUST, |c| c.0) {
                        next.insert(q, (cand, p));
                    }
                }
            }

            let updates: Vec<(String, f64, Vec<String>)> = next
                .into_iter()
                .filter(|(q, (cand, _))| best.get(*q).is_none_or(|b| *cand > b.trust))
                .map(|(q, (cand, pred))| {
                    let mut path = best[pred].path.clone();
                    path.push(q.to_string());
                    (q.to_string(), cand, path)
                })
                .collect();

            frontier.clear();
            for (q, trust, path) in updates {
                frontier.push(q.clone());
                best.insert(
                    q.clone(),
                    ProjectTrust {
                        trust,
                        hops: hop,
                        path,
                        blame: None,
                    },
                );
            }
        }

        for (id, blame) in blames {
            if let Some(p) = best.get_mut(&id) {
                p.blame = Some(blame);
            }
        }

        TrustView {
            net: self.net,
            root: self.root,
            settings: self.settings,
            now: self.now,
            projects: best,
        }
    }
}

pub struct TrustView<'a> {
    net: &'a Network,
    pub root: String,
    pub settings: Settings,
    pub now: DateTime<Utc>,
    /// Every project the root trusts at all, root included.
    pub projects: BTreeMap<String, ProjectTrust>,
}

impl TrustView<'_> {
    pub fn trust_in(&self, project: &str) -> f64 {
        self.projects.get(project).map_or(0.0, |p| p.trust)
    }

    pub fn support(&self, subject: &str, polarity: Polarity) -> Support {
        self.support_excluding(subject, polarity, &[])
    }

    /// Same as `support`, as if the `excluded` projects didn't exist. This is
    /// the building block for "would the verdict change without X?".
    pub fn support_excluding(
        &self,
        subject: &str,
        polarity: Polarity,
        excluded: &[&str],
    ) -> Support {
        let ids: Vec<&str> = self
            .projects
            .keys()
            .map(String::as_str)
            .filter(|id| *id == self.root || !excluded.contains(id))
            .collect();
        let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let node_in = |i: usize| 2 * i;
        let node_out = |i: usize| 2 * i + 1;
        let sink = 2 * ids.len();
        let source = node_out(index[self.root.as_str()]);

        let mut g = FlowGraph::new(sink + 1);
        let mut statements: BTreeMap<&str, (f64, Mark, bool, bool)> = BTreeMap::new();

        for (i, id) in ids.iter().enumerate() {
            let trust = self.projects[*id].trust;
            if *id != self.root {
                // A project can pass on no more than the root trusts it.
                g.add_edge(node_in(i), node_out(i), trust);
            }
            if let Some(state) = self.net.projects.get(*id) {
                for q in state.peers.keys() {
                    if let Some(&j) = index.get(q.as_str()) {
                        if q != &self.root {
                            g.add_edge(node_out(i), node_in(j), UNBOUNDED);
                        }
                    }
                }
            }
            if let Some(st) = self.statement(id, subject, polarity) {
                let cap = if *id == self.root { st.0 } else { trust * st.0 };
                g.add_edge(node_out(i), sink, cap);
                statements.insert(id, st);
            }
        }

        let total = g.max_flow(source, sink);
        let mut by_project: BTreeMap<&str, Vec<(Vec<String>, f64)>> = BTreeMap::new();
        for (nodes, amount) in g.decompose(source, sink) {
            // Collapse in/out pairs back to project ids; drop the sink.
            let mut path: Vec<String> = Vec::new();
            for n in nodes.iter().filter(|n| **n != sink) {
                let id = ids[n / 2];
                if path.last().map(String::as_str) != Some(id) {
                    path.push(id.to_string());
                }
            }
            let speaker = ids[nodes[nodes.len() - 2] / 2];
            by_project.entry(speaker).or_default().push((path, amount));
        }

        let mut out: Vec<Statement> = by_project
            .into_iter()
            .map(|(id, paths)| {
                let (strength, mark, provisional, under_appeal) = statements[id].clone();
                Statement {
                    project: id.to_string(),
                    amount: paths.iter().map(|(_, a)| a).sum(),
                    strength,
                    mark,
                    provisional,
                    under_appeal,
                    paths,
                }
            })
            .collect();
        out.sort_by(|a, b| b.amount.total_cmp(&a.amount));

        Support {
            polarity,
            total,
            statements: out,
        }
    }

    /// What `project` says about `subject`, as (strength, mark, provisional, under_appeal).
    fn statement(
        &self,
        project: &str,
        subject: &str,
        polarity: Polarity,
    ) -> Option<(f64, Mark, bool, bool)> {
        let user = self.net.user(project, subject)?;
        let is_root = project == self.root;
        match polarity {
            Polarity::Against => {
                let d = user.denounce.as_ref().filter(|m| m.is_active(self.now))?;
                // The root's own calls don't fade; it can lift them itself.
                let strength = if is_root {
                    1.0
                } else {
                    decay::weight(d.at, self.now, self.settings.denounce_half_life_days)
                };
                Some((strength, d.clone(), false, user.appeal.is_some()))
            }
            Polarity::For => {
                if let Some(v) = user.vouch.as_ref().filter(|m| m.is_active(self.now)) {
                    return Some((1.0, v.clone(), false, false));
                }
                // Provisional vouches never leave the project that made them.
                let p = user
                    .provisional
                    .as_ref()
                    .filter(|m| is_root && m.is_active(self.now))?;
                Some((1.0, p.clone(), true, false))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::attestation::tests::at;
    use crate::ledger::{Attestation, Kind};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// Builds a network one statement at a time.
    struct World {
        projects: Vec<(String, Vec<Attestation>)>,
    }

    impl World {
        fn new() -> Self {
            Self {
                projects: Vec::new(),
            }
        }

        fn project(&mut self, id: &str) -> &mut Vec<Attestation> {
            if let Some(i) = self.projects.iter().position(|(p, _)| p == id) {
                return &mut self.projects[i].1;
            }
            self.projects.push((id.to_string(), Vec::new()));
            &mut self.projects.last_mut().unwrap().1
        }

        fn trust(&mut self, from: &str, to: &str, w: f64) -> &mut Self {
            let mut a = Attestation::new(Kind::TrustPeer, from, to, at(0));
            a.weight = Some(w);
            self.project(from).push(a);
            self
        }

        fn say(&mut self, who: &str, kind: Kind, subject: &str, day: u32) -> &mut Self {
            let mut a = Attestation::new(kind, who, subject, at(day));
            if kind == Kind::ProvisionalVouch {
                a.expires_at = Some(at(day + 30));
            }
            self.project(who).push(a);
            self
        }

        fn net(&self) -> Network {
            let mut net = Network::new();
            for (id, atts) in &self.projects {
                net.insert(id, atts);
            }
            net
        }
    }

    fn view(net: &Network, day: u32) -> TrustView<'_> {
        TrustEngine::new(net, "root", Settings::default(), at(day)).run()
    }

    #[test]
    fn trust_decays_per_hop() {
        let mut w = World::new();
        w.trust("root", "a", 0.8)
            .trust("a", "b", 1.0)
            .trust("b", "c", 1.0);
        let net = w.net();
        let v = view(&net, 0);
        assert!(close(v.trust_in("a"), 0.8));
        assert!(close(v.trust_in("b"), 0.4));
        assert!(close(v.trust_in("c"), 0.2));
        assert_eq!(v.projects["c"].path, ["root", "a", "b", "c"]);
    }

    #[test]
    fn max_hops_is_a_hard_limit() {
        let mut w = World::new();
        w.trust("root", "a", 1.0)
            .trust("a", "b", 1.0)
            .trust("b", "c", 1.0)
            .trust("c", "d", 1.0)
            .trust("d", "e", 1.0);
        let net = w.net();
        let v = view(&net, 0);
        assert!(v.trust_in("d") > 0.0);
        assert_eq!(v.trust_in("e"), 0.0);
    }

    #[test]
    fn a_newcomer_vouched_two_hops_away_is_known() {
        // Flat Vouch would call this person "unknown" at the root.
        let mut w = World::new();
        w.trust("root", "zig", 0.9).trust("zig", "bun", 0.8).say(
            "bun",
            Kind::Vouch,
            "github:newcomer",
            0,
        );
        let net = w.net();
        let s = view(&net, 0).support("github:newcomer", Polarity::For);
        assert!(close(s.total, 0.9 * 0.8 * 0.5));
        assert_eq!(s.statements[0].paths[0].0, ["root", "zig", "bun"]);
    }

    #[test]
    fn independent_sources_add_up() {
        let mut w = World::new();
        w.trust("root", "a", 0.8)
            .trust("root", "b", 0.6)
            .say("a", Kind::Denounce, "github:spammer", 0)
            .say("b", Kind::Denounce, "github:spammer", 0);
        let net = w.net();
        let s = view(&net, 0).support("github:spammer", Polarity::Against);
        assert!(close(s.total, 1.4));
        assert_eq!(s.statements.len(), 2);
    }

    #[test]
    fn a_sybil_ring_is_capped_by_its_one_real_link() {
        // `careless` is a real project the root trusts. It trusts `s0`, the
        // entrance to a ring of 40 fake projects that all denounce an honest
        // maintainer. However many fakes there are, they share one entrance.
        let mut w = World::new();
        w.trust("root", "careless", 0.8)
            .trust("careless", "s0", 1.0);
        for i in 0..40 {
            let me = format!("s{i}");
            w.trust(&me, &format!("s{}", (i + 1) % 40), 1.0);
            w.say(&me, Kind::Denounce, "github:honest", 0);
        }
        let net = w.net();
        let v = view(&net, 0);
        let against = v.support("github:honest", Polarity::Against);

        let entrance = v.trust_in("s0");
        assert!(close(entrance, 0.4));
        assert!(against.total <= entrance + 1e-9, "got {}", against.total);

        // And one fake would have done exactly as well as forty.
        let mut w1 = World::new();
        w1.trust("root", "careless", 0.8)
            .trust("careless", "s0", 1.0)
            .say("s0", Kind::Denounce, "github:honest", 0);
        let net1 = w1.net();
        let one = view(&net1, 0).support("github:honest", Polarity::Against);
        assert!(close(one.total, against.total));
    }

    #[test]
    fn vouching_for_quarantined_users_slashes_the_voucher() {
        let mut w = World::new();
        w.trust("root", "hostile", 0.9).trust("root", "honest", 0.9);
        for i in 0..3 {
            let spammer = format!("github:spam{i}");
            w.say("hostile", Kind::Vouch, &spammer, 0);
            w.say("root", Kind::Denounce, &spammer, 1);
        }
        let net = w.net();
        let v = view(&net, 1);

        assert!(close(v.trust_in("honest"), 0.9));
        let hostile = &v.projects["hostile"];
        let expected = 0.9 * 0.7_f64.powi(3);
        assert!(
            (hostile.trust - expected).abs() < 1e-3,
            "got {}",
            hostile.trust
        );
        assert_eq!(hostile.blame.as_ref().unwrap().calls.len(), 3);
    }

    #[test]
    fn campaign_flags_count_as_quarantine_for_blame() {
        let mut w = World::new();
        w.trust("root", "p", 1.0)
            .say("p", Kind::Vouch, "github:bot", 0);
        let net = w.net();
        let plain = view(&net, 0);
        let flagged = TrustEngine::new(&net, "root", Settings::default(), at(0))
            .flag("github:bot", at(0))
            .run();
        assert!(flagged.trust_in("p") < plain.trust_in("p"));
    }

    #[test]
    fn denouncing_the_roots_people_costs_trust() {
        let mut w = World::new();
        w.trust("root", "hostile", 0.9)
            .say("root", Kind::Vouch, "github:maintainer", 0)
            .say("hostile", Kind::Denounce, "github:maintainer", 1);
        let net = w.net();
        let v = view(&net, 1);
        assert!(v.trust_in("hostile") < 0.9 * 0.71);
    }

    #[test]
    fn blame_fades_with_time() {
        let mut w = World::new();
        w.trust("root", "p", 1.0)
            .say("p", Kind::Vouch, "github:bot", 0)
            .say("root", Kind::Denounce, "github:bot", 0);
        let net = w.net();
        let fresh = view(&net, 0).trust_in("p");
        let later = view(&net, 365).trust_in("p");
        assert!(close(fresh, 0.7));
        assert!(later > 0.9);
    }

    #[test]
    fn peer_denouncements_fade_but_the_roots_own_do_not() {
        let mut w = World::new();
        w.trust("root", "p", 1.0)
            .say("p", Kind::Denounce, "github:old", 0)
            .say("root", Kind::Denounce, "github:mine", 0);
        let net = w.net();
        let v = view(&net, 180);
        assert!(close(v.support("github:old", Polarity::Against).total, 0.5));
        assert!(close(
            v.support("github:mine", Polarity::Against).total,
            1.0
        ));
    }

    #[test]
    fn provisional_vouches_stay_home() {
        let mut w = World::new();
        w.trust("root", "p", 1.0)
            .say("root", Kind::ProvisionalVouch, "github:first-pr", 0)
            .say("p", Kind::ProvisionalVouch, "github:elsewhere", 0);
        let net = w.net();
        let v = view(&net, 1);
        let mine = v.support("github:first-pr", Polarity::For);
        assert!(close(mine.total, 1.0) && mine.statements[0].provisional);
        assert!(v.support("github:elsewhere", Polarity::For).is_empty());

        // And they expire.
        assert!(
            view(&net, 31)
                .support("github:first-pr", Polarity::For)
                .is_empty()
        );
    }

    #[test]
    fn excluding_a_source_shows_what_rests_on_it() {
        let mut w = World::new();
        w.trust("root", "a", 0.9)
            .trust("root", "b", 0.3)
            .say("a", Kind::Denounce, "github:x", 0)
            .say("b", Kind::Denounce, "github:x", 0);
        let net = w.net();
        let v = view(&net, 0);
        assert!(close(v.support("github:x", Polarity::Against).total, 1.2));
        assert!(close(
            v.support_excluding("github:x", Polarity::Against, &["a"])
                .total,
            0.3
        ));
    }

    #[test]
    fn rehabilitation_lifts_a_peer_denouncement() {
        let mut w = World::new();
        w.trust("root", "p", 1.0)
            .say("p", Kind::Denounce, "github:reformed", 0)
            .say("p", Kind::Appeal, "github:reformed", 5);
        let net = w.net();
        let s = view(&net, 6).support("github:reformed", Polarity::Against);
        assert!(s.statements[0].under_appeal);

        w.say("p", Kind::Rehabilitate, "github:reformed", 10);
        let net = w.net();
        assert!(
            view(&net, 11)
                .support("github:reformed", Polarity::Against)
                .is_empty()
        );
    }
}
