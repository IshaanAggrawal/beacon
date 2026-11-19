//! Spotting coordinated waves: many different accounts opening near-identical
//! pull requests across many repos within a few days.
//!
//! Each new fingerprint is compared with everything seen in the last
//! `window_hours`. Two PRs from *different* authors are linked when
//!
//! - their text is nearly identical (copy-paste waves), or
//! - their layout is nearly identical *and* their diffs have the same shape
//!   (paraphrased waves).
//!
//! Linked PRs form clusters. A cluster only counts as a campaign once it
//! spans at least `min_authors` accounts and `min_repos` repos. One person
//! opening similar PRs, or a few people fixing the same typo, never qualifies.
//!
//! Comparison is brute force over the window. At the volumes a federation of
//! projects sees (thousands of PRs a day) that's still microseconds per PR,
//! and it's exact; LSH bucketing can replace it if that ever changes.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, TimeDelta, Utc};

use super::fingerprint::{AuthorId, Fingerprint, hamming};

#[derive(Debug, Clone, PartialEq)]
pub struct DetectorConfig {
    pub window_hours: i64,
    /// Max differing SimHash bits for two texts to count as copies.
    pub text_distance: u32,
    pub layout_distance: u32,
    /// Ignore text/layout hashes built from fewer features than this.
    pub min_text_features: u16,
    pub min_layout_features: u16,
    /// Only use layout for documentation-only PRs. Short code diffs share
    /// layouts for innocent reasons (open brace, statement, close brace);
    /// reworded prose is what layout matching is for.
    pub layout_docs_only: bool,
    pub min_authors: usize,
    pub min_repos: usize,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            window_hours: 72,
            text_distance: 6,
            layout_distance: 4,
            min_text_features: 6,
            min_layout_features: 4,
            layout_docs_only: true,
            min_authors: 4,
            min_repos: 4,
        }
    }
}

/// Everything we know about one cluster, for verdicts and explanations.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub id: usize,
    pub prs: usize,
    pub authors: usize,
    pub repos: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// Share of PRs from accounts under a month old.
    pub young_share: f64,
    pub text_links: usize,
    pub layout_links: usize,
    pub is_campaign: bool,
}

#[derive(Debug, Clone)]
struct Agg {
    prs: usize,
    authors: BTreeSet<AuthorId>,
    repos: BTreeSet<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    young: usize,
    text_links: usize,
    layout_links: usize,
}

#[derive(Debug, Clone)]
pub struct Detector {
    pub config: DetectorConfig,
    events: Vec<Fingerprint>,
    parent: Vec<usize>,
    aggs: HashMap<usize, Agg>,
    /// Index of the oldest event still inside the window.
    window_start: usize,
}

impl Detector {
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            events: Vec::new(),
            parent: Vec::new(),
            aggs: HashMap::new(),
            window_start: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Adds a PR and returns its index and its cluster as of now. PRs must
    /// arrive in time order.
    pub fn observe(&mut self, fp: Fingerprint) -> (usize, Cluster) {
        let idx = self.events.len();
        let cutoff = fp.at - TimeDelta::hours(self.config.window_hours);
        while self.window_start < idx && self.events[self.window_start].at < cutoff {
            self.window_start += 1;
        }

        self.parent.push(idx);
        self.aggs.insert(
            idx,
            Agg {
                prs: 1,
                authors: BTreeSet::from([fp.author]),
                repos: BTreeSet::from([fp.repo.clone()]),
                first_seen: fp.at,
                last_seen: fp.at,
                young: usize::from(fp.shape.young_account()),
                text_links: 0,
                layout_links: 0,
            },
        );

        let mut links = Vec::new();
        for j in self.window_start..idx {
            if let Some(kind) = self.link(&self.events[j], &fp) {
                links.push((j, kind));
            }
        }
        self.events.push(fp);
        for (j, kind) in links {
            let root = self.union(idx, j);
            let agg = self.aggs.get_mut(&root).expect("root has an aggregate");
            match kind {
                Link::Text => agg.text_links += 1,
                Link::Layout => agg.layout_links += 1,
            }
        }
        (idx, self.cluster_of(idx))
    }

    pub fn cluster_of(&mut self, idx: usize) -> Cluster {
        let root = self.find(idx);
        let a = &self.aggs[&root];
        Cluster {
            id: root,
            prs: a.prs,
            authors: a.authors.len(),
            repos: a.repos.len(),
            first_seen: a.first_seen,
            last_seen: a.last_seen,
            young_share: a.young as f64 / a.prs as f64,
            text_links: a.text_links,
            layout_links: a.layout_links,
            is_campaign: a.authors.len() >= self.config.min_authors
                && a.repos.len() >= self.config.min_repos,
        }
    }

    fn link(&self, a: &Fingerprint, b: &Fingerprint) -> Option<Link> {
        if a.author == b.author {
            return None;
        }
        let c = &self.config;
        if a.text_features.min(b.text_features) >= c.min_text_features
            && hamming(a.text, b.text) <= c.text_distance
        {
            return Some(Link::Text);
        }
        let layout_applies = !c.layout_docs_only || (a.shape.docs_only && b.shape.docs_only);
        if layout_applies
            && a.layout_features.min(b.layout_features) >= c.min_layout_features
            && hamming(a.layout, b.layout) <= c.layout_distance
            && a.shape.similar(&b.shape)
        {
            return Some(Link::Layout);
        }
        None
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) -> usize {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return ra;
        }
        // Merge the smaller aggregate into the larger one.
        let (big, small) = if self.aggs[&ra].prs >= self.aggs[&rb].prs {
            (ra, rb)
        } else {
            (rb, ra)
        };
        let s = self.aggs.remove(&small).expect("root has an aggregate");
        let b = self.aggs.get_mut(&big).expect("root has an aggregate");
        b.prs += s.prs;
        b.authors.extend(s.authors);
        b.repos.extend(s.repos);
        b.first_seen = b.first_seen.min(s.first_seen);
        b.last_seen = b.last_seen.max(s.last_seen);
        b.young += s.young;
        b.text_links += s.text_links;
        b.layout_links += s.layout_links;
        self.parent[small] = big;
        big
    }
}

#[derive(Debug, Clone, Copy)]
enum Link {
    Text,
    Layout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaign::fingerprint::{FileChange, PrEvent, Salt};
    use crate::ledger::attestation::tests::at;

    fn event(author: usize, repo: usize, hour: i64, added: &str) -> Fingerprint {
        let e = PrEvent {
            author: format!("github:user{author}"),
            repo: format!("org/repo{repo}"),
            at: at(0) + TimeDelta::hours(hour),
            title: "Update README.md".into(),
            body: String::new(),
            files: vec![FileChange {
                path: "README.md".into(),
                additions: added.lines().count() as u32,
                deletions: 0,
                hunks: 1,
            }],
            added: added.into(),
            account_created: Some(at(0)),
        };
        Fingerprint::of(&e, &Salt([1; 32]))
    }

    const SPAM: &str = "## Why this project rocks\n- Blazing fast and easy\n- Great community support\n- Check out [our site](https://spam.example)\n";
    const SPAM2: &str = "## Reasons to love it\n- Really quick, very simple\n- Helpful folks everywhere\n- Visit [this page](https://other.example)\n";

    #[test]
    fn a_paraphrased_wave_becomes_a_campaign() {
        let mut d = Detector::new(DetectorConfig::default());
        let mut last = None;
        for i in 0..6 {
            let text = if i % 2 == 0 { SPAM } else { SPAM2 };
            last = Some(d.observe(event(i, i, i as i64, text)).1);
        }
        let c = last.unwrap();
        assert!(c.is_campaign, "{c:?}");
        assert_eq!((c.authors, c.repos, c.prs), (6, 6, 6));
        assert!(c.layout_links > 0);
    }

    #[test]
    fn one_account_spamming_alone_is_not_a_campaign() {
        let mut d = Detector::new(DetectorConfig::default());
        for i in 0..10 {
            let (_, c) = d.observe(event(1, i, i as i64, SPAM));
            assert_eq!(c.prs, 1, "same author never links");
        }
    }

    #[test]
    fn independent_typo_fixes_do_not_cluster() {
        let fixes = [
            "Teh quick start guide\n",
            "Install with cargo install foo\n",
            "Fixed the broken link to docs\n",
            "Recieve -> receive in the FAQ\n",
            "Correct spelling of environment\n",
            "Typo in configuration section\n",
        ];
        let mut d = Detector::new(DetectorConfig::default());
        for (i, f) in fixes.iter().enumerate() {
            let (_, c) = d.observe(event(i, i, i as i64, f));
            assert!(!c.is_campaign);
        }
    }

    #[test]
    fn short_code_diffs_do_not_cluster_by_layout() {
        let code = "    let a = b(c);
    if x > 1 {
        run();
    }
    done();
";
        let mut d = Detector::new(DetectorConfig::default());
        for i in 0..8 {
            let mut fp = event(i, i, i as i64, code);
            // Isolate the layout signal: no usable text match.
            fp.text_features = 0;
            fp.shape.docs_only = false;
            let (_, c) = d.observe(fp);
            assert!(!c.is_campaign);
        }
    }

    #[test]
    fn links_expire_with_the_window() {
        let mut d = Detector::new(DetectorConfig::default());
        for i in 0..6 {
            // 100 hours apart: never inside the same 72h window.
            let (_, c) = d.observe(event(i, i, i as i64 * 100, SPAM));
            assert_eq!(c.prs, 1);
        }
    }
}
