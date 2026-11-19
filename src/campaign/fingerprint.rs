//! Turning a pull request into a fingerprint that can be compared across
//! projects without sharing its contents.
//!
//! A fingerprint keeps no text, no code and no handle. It has:
//!
//! - a salted hash of the author,
//! - the repo and the hour it was opened,
//! - `text`: a 64-bit SimHash of the words, which catches copy-paste waves,
//! - `layout`: a SimHash of the *kinds* of lines added (heading, bullet,
//!   link, code, prose...). Rewording a PR changes its words but usually
//!   not its layout, so this survives LLM paraphrasing,
//! - `shape`: coarse diff geometry (file types, how many files, lines, hunks)
//!   and an account-age bucket.

use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use sha2::{Digest, Sha256};

/// One file in a pull request's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: u32,
}

/// The raw pull request. It only exists long enough to be fingerprinted.
#[derive(Debug, Clone)]
pub struct PrEvent {
    /// `platform:handle`.
    pub author: String,
    pub repo: String,
    pub at: DateTime<Utc>,
    pub title: String,
    pub body: String,
    pub files: Vec<FileChange>,
    /// The added lines of the diff, joined with newlines.
    pub added: String,
    pub account_created: Option<DateTime<Utc>>,
}

/// Network-wide secret mixed into author hashes so a leaked fingerprint
/// store doesn't directly reveal handles.
#[derive(Debug, Clone)]
pub struct Salt(pub [u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AuthorId(pub [u8; 16]);

impl AuthorId {
    pub fn new(salt: &Salt, author: &str) -> Self {
        let mut h = Sha256::new();
        h.update(salt.0);
        h.update(author.to_ascii_lowercase().as_bytes());
        let digest = h.finalize();
        let mut out = [0u8; 16];
        out.copy_from_slice(&digest[..16]);
        Self(out)
    }

    pub fn short(&self) -> String {
        hex::encode(&self.0[..4])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shape {
    /// One bit per file extension (hashed into 64 slots).
    pub extensions: u64,
    pub files: u8,
    pub churn: u8,
    pub hunks: u8,
    pub docs_only: bool,
    /// 0: under a week, 1: under a month, 2: under a year, 3: older, 4: unknown.
    pub account_age: u8,
}

impl Shape {
    /// Same kinds of files and roughly the same size of change.
    pub fn similar(&self, other: &Shape) -> bool {
        self.extensions == other.extensions
            && self.docs_only == other.docs_only
            && self.files.abs_diff(other.files) <= 1
            && self.churn.abs_diff(other.churn) <= 1
            && self.hunks.abs_diff(other.hunks) <= 1
    }

    pub fn young_account(&self) -> bool {
        self.account_age <= 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub author: AuthorId,
    pub repo: String,
    /// Rounded down to the hour.
    pub at: DateTime<Utc>,
    pub text: u64,
    /// How many distinct word shingles went into `text`; tiny PRs say little.
    pub text_features: u16,
    pub layout: u64,
    pub layout_features: u16,
    pub shape: Shape,
}

const DOC_EXTENSIONS: [&str; 5] = ["md", "txt", "rst", "adoc", ""];

/// Past this many added lines a diff contains every kind of line, so all
/// long diffs share the same layout n-grams. Layout only means something
/// for short additions, which is where template spam lives anyway.
const MAX_LAYOUT_LINES: usize = 40;

impl Fingerprint {
    pub fn of(event: &PrEvent, salt: &Salt) -> Self {
        let words = tokens(&format!("{}\n{}\n{}", event.title, event.body, event.added));
        let word_shingles = shingles(&words, 3);
        let kinds: Vec<&str> = if event.added.lines().count() <= MAX_LAYOUT_LINES {
            event.added.lines().map(line_kind).collect()
        } else {
            Vec::new()
        };
        // Short additions need the 1- and 2-grams too, or a four-line wave
        // would carry almost no signal. A one-line fix still carries ~none.
        let layout_shingles: Vec<String> = (1..=3).flat_map(|k| shingles(&kinds, k)).collect();

        let extensions: Vec<String> = event.files.iter().map(|f| extension(&f.path)).collect();
        let churn: u32 = event.files.iter().map(|f| f.additions + f.deletions).sum();
        let hunks: u32 = event.files.iter().map(|f| f.hunks).sum();

        Self {
            author: AuthorId::new(salt, &event.author),
            repo: event.repo.clone(),
            at: event
                .at
                .duration_trunc(TimeDelta::hours(1))
                .unwrap_or(event.at),
            text: simhash(&word_shingles),
            text_features: distinct(&word_shingles),
            layout: simhash(&layout_shingles),
            layout_features: distinct(&layout_shingles),
            shape: Shape {
                extensions: extensions
                    .iter()
                    .fold(0, |bits, e| bits | 1 << (fnv(e.as_bytes()) % 64)),
                files: bucket(event.files.len() as u32),
                churn: bucket(churn),
                hunks: bucket(hunks),
                docs_only: !extensions.is_empty()
                    && extensions
                        .iter()
                        .all(|e| DOC_EXTENSIONS.contains(&e.as_str())),
                account_age: match event.account_created {
                    None => 4,
                    Some(c) => match (event.at - c).num_days() {
                        ..7 => 0,
                        7..30 => 1,
                        30..365 => 2,
                        _ => 3,
                    },
                },
            },
        }
    }
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// log2 bucket: 0, 1, 2-3, 4-7, 8-15, ...
fn bucket(n: u32) -> u8 {
    (32 - n.leading_zeros()) as u8
}

fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// What a line *is*, ignoring what it says.
fn line_kind(line: &str) -> &'static str {
    let t = line.trim();
    if t.is_empty() {
        "blank"
    } else if t.starts_with("```") {
        "fence"
    } else if t.starts_with('#') && !t.starts_with("#[") && !t.starts_with("#!") {
        "heading"
    } else if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        if t.contains("](") {
            "bullet-link"
        } else {
            "bullet"
        }
    } else if t
        .split_once(". ")
        .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
    {
        "numbered"
    } else if t.starts_with('|') {
        "table"
    } else if t.starts_with("//") || t.starts_with("/*") || t.starts_with("<!--") {
        "comment"
    } else if t.contains("](") || t.contains("http://") || t.contains("https://") {
        "link"
    } else if t.ends_with(';') || t.ends_with('{') || t.ends_with('}') || t.contains(" = ") {
        "code"
    } else if t.len() > 80 {
        "prose-long"
    } else {
        "prose"
    }
}

fn shingles<T: AsRef<str>>(items: &[T], k: usize) -> Vec<String> {
    if items.len() < k {
        return items.iter().map(|s| s.as_ref().to_string()).collect();
    }
    items
        .windows(k)
        .map(|w| w.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" "))
        .collect()
}

fn distinct(items: &[String]) -> u16 {
    let mut v: Vec<&String> = items.iter().collect();
    v.sort();
    v.dedup();
    v.len().min(u16::MAX as usize) as u16
}

/// Charikar's SimHash: similar feature sets give hashes a small Hamming distance apart.
fn simhash(features: &[String]) -> u64 {
    let mut weights = [0i32; 64];
    for f in features {
        let h = mix(fnv(f.as_bytes()));
        for (bit, w) in weights.iter_mut().enumerate() {
            *w += if h >> bit & 1 == 1 { 1 } else { -1 };
        }
    }
    weights
        .iter()
        .enumerate()
        .fold(0, |acc, (bit, w)| if *w > 0 { acc | 1 << bit } else { acc })
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// SplitMix64 finalizer, to spread FNV's output across all 64 bits.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::attestation::tests::at;

    fn salt() -> Salt {
        Salt([7; 32])
    }

    fn pr(author: &str, title: &str, added: &str, path: &str) -> PrEvent {
        PrEvent {
            author: author.into(),
            repo: "org/repo".into(),
            at: at(10),
            title: title.into(),
            body: String::new(),
            files: vec![FileChange {
                path: path.into(),
                additions: added.lines().count() as u32,
                deletions: 0,
                hunks: 1,
            }],
            added: added.into(),
            account_created: Some(at(8)),
        }
    }

    const ORIGINAL: &str = "## Features\n- Fast startup with zero config\n- Works offline\n- See the [guide](https://example.com/guide)\n";
    const PARAPHRASE: &str = "## Key capabilities\n- Launches instantly, no setup needed\n- Functions without a network\n- Read the [handbook](https://example.org/docs)\n";
    const UNRELATED: &str = "fn parse(input: &str) -> Result<Ast, Error> {\n    let tokens = lex(input)?;\n    build(tokens)\n}\n";

    #[test]
    fn copies_have_identical_text_hashes() {
        let a = Fingerprint::of(
            &pr("github:a", "Improve README", ORIGINAL, "README.md"),
            &salt(),
        );
        let b = Fingerprint::of(
            &pr("github:b", "Improve README", ORIGINAL, "README.md"),
            &salt(),
        );
        assert_eq!(hamming(a.text, b.text), 0);
        assert_ne!(a.author, b.author);
    }

    #[test]
    fn a_paraphrase_changes_the_words_but_not_the_layout() {
        let a = Fingerprint::of(
            &pr("github:a", "Improve README", ORIGINAL, "README.md"),
            &salt(),
        );
        let b = Fingerprint::of(
            &pr(
                "github:b",
                "Docs: add capabilities",
                PARAPHRASE,
                "README.md",
            ),
            &salt(),
        );
        let c = Fingerprint::of(
            &pr("github:c", "Refactor parser", UNRELATED, "src/parse.rs"),
            &salt(),
        );

        assert!(hamming(a.text, b.text) > 12, "text should look unrelated");
        assert_eq!(hamming(a.layout, b.layout), 0, "layout should match");
        assert!(a.shape.similar(&b.shape));
        assert!(!a.shape.similar(&c.shape));
    }

    #[test]
    fn author_hash_depends_on_the_salt_and_ignores_case() {
        let a = AuthorId::new(&salt(), "github:Alice");
        assert_eq!(a, AuthorId::new(&salt(), "github:alice"));
        assert_ne!(a, AuthorId::new(&Salt([8; 32]), "github:alice"));
    }

    #[test]
    fn records_coarse_shape_only() {
        let f = Fingerprint::of(&pr("github:a", "t", ORIGINAL, "docs/README.md"), &salt());
        assert!(f.shape.docs_only);
        assert_eq!(f.shape.account_age, 0);
        assert_eq!(f.shape.churn, bucket(4));
        assert_eq!(f.at.format("%M:%S").to_string(), "00:00");
    }

    #[test]
    fn long_diffs_carry_no_layout_signal() {
        let long = "let x = 1;
"
        .repeat(MAX_LAYOUT_LINES + 1);
        let f = Fingerprint::of(&pr("github:a", "t", &long, "src/a.rs"), &salt());
        assert_eq!(f.layout_features, 0);
    }

    #[test]
    fn line_kinds() {
        assert_eq!(line_kind("## Title"), "heading");
        assert_eq!(line_kind("#[derive(Debug)]"), "prose");
        assert_eq!(line_kind("- [x](y)"), "bullet-link");
        assert_eq!(line_kind("12. step"), "numbered");
        assert_eq!(line_kind("let x = 1;"), "code");
    }
}
