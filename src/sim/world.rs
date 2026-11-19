//! A seeded, synthetic federation for the benchmark and the demo.
//!
//! Ghostty's real list is included for its structure (397 vouches, 28
//! denouncements, real dates), but every handle is replaced with a
//! pseudonym: the simulator invents PR traffic, and invented activity
//! shouldn't be attached to real people's names.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::rng::Rng;
use crate::campaign::{FileChange, PrEvent};
use crate::graph::Network;
use crate::ledger::import::{import_vouch_list, parse_history};
use crate::ledger::vouch_td::VouchList;
use crate::ledger::{Attestation, Kind};

const GHOSTTY_LIST: &str = include_str!("../../data/ghostty/VOUCHED.td");
const GHOSTTY_HISTORY: &str = include_str!("../../data/ghostty/history.jsonl");

pub const GHOSTTY: &str = "ghostty";
pub const HOSTILE: &str = "quickforge";
pub const CARELESS: &str = "drift";

const PROJECT_NAMES: [&str; 29] = [
    "tidepool", "lantern", "quarry", "foxglove", "basalt", "meridian", "sparrow", "kelp",
    "juniper", "orchard", "cinder", "halyard", "pebble", "wicket", "marlin", "thistle", "copper",
    "drift", "ember", "fjord", "garnet", "heron", "indigo", "jetty", "kestrel", "loam", "moss",
    "nectar", "oriole",
];
const LANGS: [&str; 6] = ["rs", "go", "ts", "py", "zig", "c"];

/// What an account really is. The detector and trust graph never see this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Truth {
    /// Established, honest contributor.
    Regular,
    /// Honest account nobody has vouched for yet.
    Newcomer,
    /// Part of a coordinated campaign.
    Spam,
    /// Denounced somewhere before for low-effort PRs; acting alone.
    RepeatOffender,
}

impl Truth {
    pub fn is_bad(self) -> bool {
        matches!(self, Truth::Spam | Truth::RepeatOffender)
    }
}

#[derive(Debug, Clone)]
pub struct SimPr {
    pub event: PrEvent,
    /// Project receiving the PR.
    pub target: String,
    pub truth: Truth,
    /// Which campaign, for spam.
    pub campaign: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct WorldConfig {
    pub seed: u64,
    pub regulars: usize,
    pub regular_prs: usize,
    pub newcomers: usize,
    pub copy_paste_accounts: usize,
    pub paraphrase_accounts: usize,
    pub campaign_repos: usize,
    pub sybils: usize,
    pub repeat_offender_prs: usize,
    /// Chance that each structural detail of a paraphrased spam PR is also
    /// varied (bullet count, blank lines, link placement, intro length):
    /// an adversary who rewords *and* reshapes. 0 means words only.
    pub layout_jitter: f64,
    pub days: i64,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            regulars: 2000,
            regular_prs: 1500,
            newcomers: 200,
            copy_paste_accounts: 25,
            paraphrase_accounts: 35,
            campaign_repos: 25,
            sybils: 40,
            repeat_offender_prs: 40,
            layout_jitter: 0.0,
            days: 14,
        }
    }
}

#[derive(Debug, Clone)]
pub struct World {
    pub config: WorldConfig,
    pub start: DateTime<Utc>,
    pub ledgers: BTreeMap<String, Vec<Attestation>>,
    /// Real, honest projects (Ghostty and `drift` included).
    pub honest_projects: Vec<String>,
    pub sybils: Vec<String>,
    pub truth: HashMap<String, Truth>,
    /// Honest people the hostile project and Sybil ring denounce.
    pub smeared: BTreeSet<String>,
    pub prs: Vec<SimPr>,
}

struct Builder {
    rng: Rng,
    start: DateTime<Utc>,
    ledgers: BTreeMap<String, Vec<Attestation>>,
    truth: HashMap<String, Truth>,
    created: HashMap<String, DateTime<Utc>>,
    prs: Vec<SimPr>,
}

impl Builder {
    fn say(&mut self, issuer: &str, kind: Kind, subject: &str, reason: Option<&str>) {
        let at = self.start - TimeDelta::hours(self.rng.range(24, 300 * 24) as i64);
        let mut a = Attestation::new(kind, issuer, subject, at);
        a.reason = reason.map(str::to_string);
        self.ledgers.entry(issuer.to_string()).or_default().push(a);
    }

    fn trust(&mut self, from: &str, to: &str, weight: f64) {
        let at = self.start - TimeDelta::days(400);
        let mut a = Attestation::new(Kind::TrustPeer, from, to, at);
        a.weight = Some((weight * 100.0).round() / 100.0);
        self.ledgers.entry(from.to_string()).or_default().push(a);
    }

    fn account(&mut self, handle: &str, truth: Truth, age_days: i64) {
        self.truth.insert(handle.to_string(), truth);
        self.created
            .insert(handle.to_string(), self.start - TimeDelta::days(age_days));
    }

    fn pr(
        &mut self,
        author: &str,
        target: &str,
        at: DateTime<Utc>,
        content: Content,
        campaign: Option<u8>,
    ) {
        let truth = self.truth[author];
        self.prs.push(SimPr {
            event: PrEvent {
                author: author.to_string(),
                repo: target.to_string(),
                at,
                title: content.title,
                body: content.body,
                files: content.files,
                added: content.added,
                account_created: self.created.get(author).copied(),
            },
            target: target.to_string(),
            truth,
            campaign,
        });
    }

    fn time_in(&mut self, from_hour: i64, span_hours: i64) -> DateTime<Utc> {
        self.start
            + TimeDelta::hours(from_hour)
            + TimeDelta::minutes(self.rng.range(0, (span_hours.max(1) * 60) as usize) as i64)
    }
}

impl World {
    pub fn generate(config: WorldConfig) -> Self {
        let start = Utc.with_ymd_and_hms(2026, 11, 19, 0, 0, 0).unwrap();
        let mut b = Builder {
            rng: Rng::new(config.seed),
            start,
            ledgers: BTreeMap::new(),
            truth: HashMap::new(),
            created: HashMap::new(),
            prs: Vec::new(),
        };
        let hours = config.days * 24;

        // --- Projects and who trusts whom ---------------------------------
        let mut honest: Vec<String> = vec![GHOSTTY.to_string()];
        honest.extend(PROJECT_NAMES.iter().map(|s| s.to_string()));
        let lang: HashMap<String, &str> = honest
            .iter()
            .map(|p| (p.clone(), *b.rng.pick(&LANGS)))
            .collect();

        for p in &honest {
            let peers = b.rng.range(2, 6);
            let mut chosen = BTreeSet::new();
            while chosen.len() < peers {
                let q = &honest[b.rng.skewed(honest.len())];
                if q != p {
                    chosen.insert(q.clone());
                }
            }
            for q in chosen {
                let w = b.rng.between(0.4, 0.95);
                b.trust(p, &q, w);
            }
        }
        // The hostile project looks legitimate and has earned some trust.
        b.trust(GHOSTTY, HOSTILE, 0.7);
        for _ in 0..2 {
            let p = honest[b.rng.range(1, honest.len())].clone();
            let w = b.rng.between(0.6, 0.8);
            b.trust(&p, HOSTILE, w);
        }
        let sybils: Vec<String> = (0..config.sybils)
            .map(|i| format!("mirror-{i:02}"))
            .collect();
        if !sybils.is_empty() {
            // One careless real project lets the ring in through one edge.
            b.trust(CARELESS, &sybils[0], 0.6);
            for (i, s) in sybils.iter().enumerate() {
                b.trust(s, &sybils[(i + 1) % sybils.len()], 1.0);
                for _ in 0..2 {
                    let other = sybils[b.rng.range(0, sybils.len())].clone();
                    if &other != s {
                        b.trust(s, &other, 1.0);
                    }
                }
            }
        }

        // --- Ghostty's real list, pseudonymized ---------------------------
        let list = VouchList::parse(GHOSTTY_LIST, "github").expect("fixture parses");
        let history = parse_history(GHOSTTY_HISTORY, "github").expect("fixture parses");
        let real = import_vouch_list(
            &list,
            &history,
            GHOSTTY,
            "VOUCHED.td",
            start - TimeDelta::days(277),
        );
        let mut ghostty_vouched = Vec::new();
        let mut ghostty_denounced = Vec::new();
        for (i, mut a) in real.into_iter().enumerate() {
            let pseudo = match a.kind {
                Kind::Denounce => format!("github:gh-d{i:03}"),
                _ => format!("github:gh-v{i:03}"),
            };
            a.subject = pseudo.clone();
            if a.kind == Kind::Denounce {
                ghostty_denounced.push(pseudo);
            } else {
                ghostty_vouched.push(pseudo);
            }
            b.ledgers.entry(GHOSTTY.into()).or_default().push(a);
        }

        // --- Accounts -----------------------------------------------------
        let mut regulars: Vec<String> = (0..config.regulars)
            .map(|i| format!("github:u{i:04}"))
            .collect();
        regulars.extend(ghostty_vouched.iter().cloned());
        b.rng.shuffle(&mut regulars);
        for r in &regulars {
            let age = b.rng.range(400, 4000) as i64;
            b.account(r, Truth::Regular, age);
        }
        let newcomers: Vec<String> = (0..config.newcomers)
            .map(|i| format!("github:n{i:03}"))
            .collect();
        for n in &newcomers {
            let age = if b.rng.chance(0.5) {
                b.rng.range(0, 30)
            } else {
                b.rng.range(30, 2000)
            };
            b.account(n, Truth::Newcomer, age as i64);
        }
        let mut offenders: Vec<String> = (0..40).map(|i| format!("github:ro{i:02}")).collect();
        offenders.extend(ghostty_denounced.iter().cloned());
        for o in &offenders {
            let age = b.rng.range(60, 1500) as i64;
            b.account(o, Truth::RepeatOffender, age);
        }
        let copy_paste: Vec<String> = (0..config.copy_paste_accounts)
            .map(|i| format!("github:cp{i:02}"))
            .collect();
        let paraphrase: Vec<String> = (0..config.paraphrase_accounts)
            .map(|i| format!("github:pp{i:02}"))
            .collect();
        for s in copy_paste.iter().chain(&paraphrase) {
            // Real spam-labelled PRs: 19-40% come from accounts under a
            // month old; the rest are older (see docs/BENCHMARKS.md).
            let age = if b.rng.chance(0.3) {
                b.rng.range(0, 30)
            } else {
                b.rng.range(30, 2500)
            };
            b.account(s, Truth::Spam, age as i64);
        }
        let campaign: Vec<String> = copy_paste.iter().chain(&paraphrase).cloned().collect();

        // --- What each project has said -----------------------------------
        for p in honest.iter().skip(1) {
            let n = b.rng.range(40, 160);
            for _ in 0..n {
                let r = regulars[b.rng.skewed(regulars.len())].clone();
                b.say(p, Kind::Vouch, &r, None);
            }
        }
        for o in offenders.iter().take(40) {
            let n = b.rng.range(1, 4);
            for _ in 0..n {
                let p = honest[b.rng.range(1, honest.len())].clone();
                b.say(
                    &p,
                    Kind::Denounce,
                    o,
                    Some("repeated low-effort generated PRs"),
                );
            }
        }
        for o in &ghostty_denounced {
            if b.rng.chance(0.3) {
                let p = honest[b.rng.range(1, honest.len())].clone();
                b.say(&p, Kind::Denounce, o, Some("ignored our AI policy"));
            }
        }
        // drift got fooled by two campaign accounts before the wave started.
        for s in b.rng.sample(&campaign, 2) {
            b.say(CARELESS, Kind::Vouch, &s, None);
        }

        // Smear targets: some of Ghostty's own people, a few others, and some newcomers.
        let mut smeared: BTreeSet<String> = BTreeSet::new();
        smeared.extend(b.rng.sample(&ghostty_vouched, 8));
        smeared.extend(b.rng.sample(&regulars[..300], 4));
        smeared.extend(b.rng.sample(&newcomers, 5));

        for s in b.rng.sample(&campaign, 20) {
            b.say(HOSTILE, Kind::Vouch, &s, None);
        }
        for r in b.rng.sample(&regulars, 30) {
            b.say(HOSTILE, Kind::Vouch, &r, None);
        }
        for t in smeared.clone() {
            b.say(HOSTILE, Kind::Denounce, &t, Some("spam"));
        }
        for s in &sybils {
            for c in &campaign {
                b.say(s, Kind::Vouch, c, None);
            }
            for t in smeared.clone() {
                b.say(s, Kind::Denounce, &t, Some("bot account"));
            }
        }

        // --- Honest traffic -----------------------------------------------
        let vouchers: HashMap<String, Vec<String>> = {
            let mut m: HashMap<String, Vec<String>> = HashMap::new();
            for (p, atts) in &b.ledgers {
                if !honest.contains(p) {
                    continue;
                }
                for a in atts.iter().filter(|a| a.kind == Kind::Vouch) {
                    m.entry(a.subject.clone()).or_default().push(p.clone());
                }
            }
            m
        };
        for _ in 0..config.regular_prs {
            let author = regulars[b.rng.skewed(regulars.len())].clone();
            let target = match vouchers.get(&author) {
                Some(ps) if b.rng.chance(0.7) => b.rng.pick(ps).clone(),
                _ => honest[b.rng.skewed(honest.len())].clone(),
            };
            let at = b.time_in(0, hours);
            let roll = b.rng.unit();
            let content = if roll < 0.65 {
                code_pr(&mut b.rng, lang[&target])
            } else if roll < 0.8 {
                docs_addition(&mut b.rng)
            } else {
                docs_fix(&mut b.rng)
            };
            b.pr(&author, &target, at, content, None);
        }
        // Everyone who was smeared keeps contributing.
        for author in smeared.clone() {
            let target = if b.rng.chance(0.5) {
                GHOSTTY.to_string()
            } else {
                honest[b.rng.range(0, honest.len())].clone()
            };
            let at = b.time_in(0, hours);
            let content = code_pr(&mut b.rng, lang[&target]);
            b.pr(&author, &target, at, content, None);
        }
        for n in &newcomers {
            for _ in 0..if b.rng.chance(0.8) { 1 } else { 2 } {
                let target = honest[b.rng.skewed(honest.len())].clone();
                let at = b.time_in(0, hours);
                let roll = b.rng.unit();
                let content = if roll < 0.4 {
                    docs_fix(&mut b.rng)
                } else if roll < 0.7 {
                    small_code_fix(&mut b.rng, lang[&target])
                } else {
                    docs_addition(&mut b.rng)
                };
                b.pr(n, &target, at, content, None);
            }
        }
        // An honest docs sprint: newcomers adding similar example sections
        // to two repos at once. Looks coordinated; isn't spam.
        let sprint_repos = [honest[1].clone(), honest[2].clone()];
        for n in b.rng.sample(&newcomers, 8) {
            let target = b.rng.pick(&sprint_repos).clone();
            let at = b.time_in(5 * 24, 48);
            let content = sprint_example(&mut b.rng);
            b.pr(&n, &target, at, content, None);
        }
        // Like real "ai-slop" PRs: established accounts dumping large,
        // multi-file changes, mostly back into the same repo.
        let home: HashMap<String, String> = offenders
            .iter()
            .map(|o| (o.clone(), honest[b.rng.range(0, honest.len())].clone()))
            .collect();
        for _ in 0..config.repeat_offender_prs {
            let o = offenders[b.rng.range(0, offenders.len())].clone();
            let target = if b.rng.chance(0.8) {
                home[&o].clone()
            } else {
                honest[b.rng.range(0, honest.len())].clone()
            };
            let at = b.time_in(0, hours);
            let content = slop_pr(&mut b.rng);
            b.pr(&o, &target, at, content, None);
        }

        // --- Campaigns ----------------------------------------------------
        let targets = b
            .rng
            .sample(&honest, config.campaign_repos.min(honest.len()));
        let jitter = config.layout_jitter;
        let wave = |b: &mut Builder, accounts: &[String], from_hour: i64, span: i64, id: u8| {
            for a in accounts {
                let n = b.rng.range(1, 4);
                for t in b.rng.sample(&targets, n) {
                    let at = b.time_in(from_hour, span);
                    let content = if id == 0 {
                        copy_paste_spam()
                    } else {
                        paraphrased_spam(&mut b.rng, jitter)
                    };
                    b.pr(a, &t, at, content, Some(id));
                }
            }
        };
        wave(&mut b, &copy_paste, 3 * 24, 24, 0);
        wave(&mut b, &paraphrase, 8 * 24, 48, 1);

        b.prs.sort_by_key(|p| p.event.at);
        World {
            config,
            start,
            ledgers: b.ledgers,
            honest_projects: honest,
            sybils,
            truth: b.truth,
            smeared,
            prs: b.prs,
        }
    }

    pub fn network(&self) -> Network {
        let mut net = Network::new();
        for (id, atts) in &self.ledgers {
            net.insert(id, atts);
        }
        net
    }
}

// --- PR content ------------------------------------------------------------

pub struct Content {
    pub title: String,
    pub body: String,
    pub files: Vec<FileChange>,
    pub added: String,
}

const WORDS: [&str; 64] = [
    "parser",
    "buffer",
    "config",
    "render",
    "cursor",
    "socket",
    "thread",
    "cache",
    "token",
    "layout",
    "font",
    "glyph",
    "shader",
    "window",
    "event",
    "queue",
    "timer",
    "input",
    "output",
    "stream",
    "frame",
    "scroll",
    "theme",
    "plugin",
    "handler",
    "error",
    "state",
    "palette",
    "terminal",
    "session",
    "process",
    "signal",
    "memory",
    "allocator",
    "index",
    "range",
    "slice",
    "channel",
    "logger",
    "metric",
    "schema",
    "option",
    "flag",
    "path",
    "file",
    "line",
    "column",
    "width",
    "height",
    "offset",
    "length",
    "batch",
    "retry",
    "limit",
    "timeout",
    "encoder",
    "decoder",
    "header",
    "footer",
    "table",
    "cell",
    "row",
    "unicode",
    "locale",
];
const VERBS: [&str; 12] = [
    "fix", "handle", "avoid", "support", "clamp", "reuse", "skip", "cache", "validate", "track",
    "reset", "flush",
];

fn words(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|_| *rng.pick(&WORDS))
        .collect::<Vec<_>>()
        .join(" ")
}

fn words_in(rng: &mut Rng, lo: usize, hi: usize) -> String {
    let n = rng.range(lo, hi);
    words(rng, n)
}

fn code_pr(rng: &mut Rng, ext: &str) -> Content {
    let nfiles = rng.range(1, 7);
    let mut files = Vec::new();
    let mut added = String::new();
    for _ in 0..nfiles {
        let lines = rng.range(3, 80);
        for _ in 0..lines {
            let line = match rng.range(0, 5) {
                0 => format!(
                    "    let {} = {}({});",
                    rng.pick(&WORDS),
                    rng.pick(&WORDS),
                    rng.pick(&WORDS)
                ),
                1 => format!(
                    "    if {}.{} > {} {{",
                    rng.pick(&WORDS),
                    rng.pick(&WORDS),
                    rng.range(0, 99)
                ),
                2 => "    }".to_string(),
                3 => format!("    // {} {}", rng.pick(&VERBS), words(rng, 4)),
                _ => format!("    {}.{}();", rng.pick(&WORDS), rng.pick(&VERBS)),
            };
            added.push_str(&line);
            added.push('\n');
        }
        files.push(FileChange {
            path: format!("src/{}.{ext}", rng.pick(&WORDS)),
            additions: lines as u32,
            deletions: rng.range(0, lines + 1) as u32,
            hunks: rng.range(1, 6) as u32,
        });
    }
    Content {
        title: format!(
            "{} {} in {}",
            rng.pick(&VERBS),
            words(rng, 2),
            rng.pick(&WORDS)
        ),
        body: words_in(rng, 8, 40),
        files,
        added,
    }
}

fn small_code_fix(rng: &mut Rng, ext: &str) -> Content {
    let added = format!(
        "    if {}.is_empty() {{\n        return Err({}::{});\n    }}\n",
        rng.pick(&WORDS),
        rng.pick(&WORDS),
        rng.pick(&WORDS)
    );
    Content {
        title: format!("{} empty {}", rng.pick(&VERBS), rng.pick(&WORDS)),
        body: words_in(rng, 5, 20),
        files: vec![FileChange {
            path: format!("src/{}.{ext}", rng.pick(&WORDS)),
            additions: 3,
            deletions: rng.range(0, 3) as u32,
            hunks: 1,
        }],
        added,
    }
}

const DEFAULT_TITLES: [&str; 4] = [
    "Update README.md",
    "Fix typo",
    "docs: fix typo",
    "Update docs",
];

fn docs_fix(rng: &mut Rng) -> Content {
    let lines = rng.range(1, 3);
    let added: String = (0..lines).map(|_| words_in(rng, 5, 14) + "\n").collect();
    Content {
        title: rng.pick(&DEFAULT_TITLES).to_string(),
        body: if rng.chance(0.5) {
            String::new()
        } else {
            words(rng, 6)
        },
        files: vec![FileChange {
            path: if rng.chance(0.6) {
                "README.md".into()
            } else {
                format!("docs/{}.md", rng.pick(&WORDS))
            },
            additions: lines as u32,
            deletions: lines as u32,
            hunks: 1,
        }],
        added,
    }
}

fn docs_addition(rng: &mut Rng) -> Content {
    let lines = rng.range(5, 26);
    let mut added = String::new();
    for _ in 0..lines {
        let line = match rng.range(0, 6) {
            0 => format!("## {}", words(rng, 2)),
            1 => format!("- {}", words_in(rng, 3, 9)),
            2 => String::new(),
            3 => format!("```{}", rng.pick(&["", "sh", "toml"])),
            _ => words_in(rng, 6, 20),
        };
        added.push_str(&line);
        added.push('\n');
    }
    Content {
        title: format!("docs: document {}", words(rng, 2)),
        body: words_in(rng, 4, 20),
        files: vec![FileChange {
            path: format!("docs/{}.md", rng.pick(&WORDS)),
            additions: lines as u32,
            deletions: 0,
            hunks: rng.range(1, 3) as u32,
        }],
        added,
    }
}

fn sprint_example(rng: &mut Rng) -> Content {
    let added = format!(
        "## Example: {}\n\n{}\n\n```sh\n{} --{} {}\n```\n",
        words(rng, 2),
        words(rng, 12),
        rng.pick(&WORDS),
        rng.pick(&WORDS),
        rng.pick(&WORDS)
    );
    Content {
        title: format!("docs: add {} example", rng.pick(&WORDS)),
        body: "Part of the docs sprint.".into(),
        files: vec![FileChange {
            path: "docs/examples.md".into(),
            additions: 7,
            deletions: 0,
            hunks: 1,
        }],
        added,
    }
}

fn slop_pr(rng: &mut Rng) -> Content {
    const EXTS: [&str; 6] = ["py", "ts", "md", "json", "yml", "toml"];
    let nfiles = rng.range(2, 11);
    let mut files = Vec::new();
    let mut added = String::new();
    for _ in 0..nfiles {
        let lines = rng.range(5, 60);
        for _ in 0..lines {
            added.push_str(&words_in(rng, 3, 12));
            added.push('\n');
        }
        files.push(FileChange {
            path: format!(
                "{}/{}.{}",
                rng.pick(&WORDS),
                rng.pick(&WORDS),
                rng.pick(&EXTS)
            ),
            additions: lines as u32,
            deletions: rng.range(0, lines) as u32,
            hunks: rng.range(1, 5) as u32,
        });
    }
    Content {
        title: format!("Refactor and improve {}", words(rng, 2)),
        body: words_in(rng, 40, 120),
        files,
        added,
    }
}

fn copy_paste_spam() -> Content {
    let added = "## Why you should use this\n\nThis project is amazing and easy to use for everyone.\n- Fast and reliable\n- Beginner friendly\n- Great documentation\n- Learn more at [our blog](https://example.invalid/blog)\n";
    Content {
        title: "Update README.md".into(),
        body: "I have improved the README to make it more clear and helpful for new users.".into(),
        files: vec![FileChange {
            path: "README.md".into(),
            additions: 7,
            deletions: 0,
            hunks: 1,
        }],
        added: added.into(),
    }
}

/// Same structure every time, different words every time: what an LLM
/// asked to "rewrite this so it doesn't look duplicated" produces.
fn paraphrased_spam(rng: &mut Rng, jitter: f64) -> Content {
    const HEADINGS: [&str; 5] = [
        "## Key benefits",
        "## Why it matters",
        "## Highlights",
        "## What makes it great",
        "## Advantages",
    ];
    const INTROS: [&str; 5] = [
        "This library delivers a smooth experience for developers of all levels.",
        "A thoughtfully designed tool that simplifies everyday workflows.",
        "Built to help teams move faster with less configuration overhead.",
        "An intuitive solution that streamlines common development tasks.",
        "Designed from the ground up to be approachable and productive.",
    ];
    const POINTS: [&str; 12] = [
        "Robust performance under load",
        "Clear and concise APIs",
        "Seamless integration",
        "Excellent developer ergonomics",
        "Thorough documentation coverage",
        "Minimal setup required",
        "Active and welcoming maintainers",
        "Cross-platform support",
        "Predictable release cadence",
        "Well-tested core modules",
        "Flexible configuration",
        "Lightweight footprint",
    ];
    const LINKS: [&str; 4] = [
        "- Explore the [full guide](https://example.invalid/guide)",
        "- See [this overview](https://example.invalid/overview)",
        "- Read [our write-up](https://example.invalid/post)",
        "- Check the [resource page](https://example.invalid/res)",
    ];
    const TITLES: [&str; 5] = [
        "docs: enhance README",
        "Improve project description",
        "Add benefits section",
        "README: clarify value proposition",
        "Documentation improvements",
    ];
    let bullets = if rng.chance(jitter) {
        rng.range(2, 6)
    } else {
        3
    };
    let mut lines: Vec<String> = vec![rng.pick(&HEADINGS).to_string()];
    if !rng.chance(jitter) {
        lines.push(String::new());
    }
    lines.push(rng.pick(&INTROS).to_string());
    if rng.chance(jitter) {
        lines.push(rng.pick(&INTROS).to_string());
    }
    let mut points: Vec<String> = rng
        .sample(&POINTS, bullets)
        .iter()
        .map(|p| format!("- {p}"))
        .collect();
    let link = rng.pick(&LINKS).to_string();
    if rng.chance(jitter) {
        points.insert(0, link);
    } else {
        points.push(link);
    }
    lines.extend(points);
    let added = lines.join(
        "
",
    ) + "
";
    Content {
        title: rng.pick(&TITLES).to_string(),
        body: rng.pick(&INTROS).to_string(),
        files: vec![FileChange {
            path: "README.md".into(),
            additions: lines.len() as u32,
            deletions: 0,
            hunks: 1,
        }],
        added,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let a = World::generate(WorldConfig::default());
        let b = World::generate(WorldConfig::default());
        assert_eq!(a.prs.len(), b.prs.len());
        assert!(
            a.prs
                .iter()
                .zip(&b.prs)
                .all(|(x, y)| x.event.author == y.event.author && x.event.at == y.event.at)
        );
    }

    #[test]
    fn has_every_population() {
        let w = World::generate(WorldConfig::default());
        for t in [
            Truth::Regular,
            Truth::Newcomer,
            Truth::Spam,
            Truth::RepeatOffender,
        ] {
            assert!(w.prs.iter().any(|p| p.truth == t), "{t:?} missing");
        }
        assert!(w.prs.windows(2).all(|p| p[0].event.at <= p[1].event.at));
        assert!(
            !w.ledgers[GHOSTTY]
                .iter()
                .any(|a| a.subject == "github:mitchellh"),
            "real handles are pseudonymized"
        );
    }
}
