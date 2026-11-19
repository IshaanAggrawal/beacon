//! Reader for Vouch's `VOUCHED.td` files (<https://github.com/mitchellh/vouch>).
//!
//! The format is one handle per line:
//!
//! ```text
//! # comment
//! alice
//! codeberg:bob
//! -mallory reason for the denouncement
//! ```
//!
//! A leading `-` denounces, an optional `platform:` prefix names the forge,
//! and anything after the first run of whitespace is a free-text reason.

use std::fmt;

use thiserror::Error;

/// Whether a list vouches for someone or has denounced them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Vouched,
    Denounced,
}

/// A user handle qualified by the forge it lives on, e.g. `github:alice`.
///
/// Handles are lowercased: GitHub, GitLab and Codeberg all treat them
/// case-insensitively, and Ghostty's own list is stored lowercase while its
/// commit messages use the original casing (`@MrMage` vs `mrmage`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle {
    pub platform: String,
    pub name: String,
}

impl Handle {
    pub fn new(platform: &str, name: &str) -> Self {
        Self {
            platform: platform.to_ascii_lowercase(),
            name: name.trim_start_matches('@').to_ascii_lowercase(),
        }
    }

    /// Parses `name` or `platform:name`, using `default_platform` for the first form.
    pub fn parse(raw: &str, default_platform: &str) -> Self {
        match raw.split_once(':') {
            Some((platform, name)) => Self::new(platform, name),
            None => Self::new(default_platform, raw),
        }
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.platform, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub handle: Handle,
    pub standing: Standing,
    pub reason: Option<String>,
    /// 1-based line number in the source file, for error messages and evidence.
    pub line: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("line {line}: expected a handle after '-'")]
    MissingHandle { line: usize },
    #[error("line {line}: '{raw}' has an empty platform or name")]
    MalformedHandle { line: usize, raw: String },
}

#[derive(Debug, Clone, Default)]
pub struct VouchList {
    pub entries: Vec<Entry>,
}

impl VouchList {
    pub fn parse(src: &str, default_platform: &str) -> Result<Self, ParseError> {
        let mut entries = Vec::new();

        for (idx, raw_line) in src.lines().enumerate() {
            let line = idx + 1;
            let text = raw_line.trim();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }

            let (standing, rest) = match text.strip_prefix('-') {
                Some(rest) => (Standing::Denounced, rest.trim_start()),
                None => (Standing::Vouched, text),
            };

            let (raw_handle, reason) = match rest.split_once(char::is_whitespace) {
                Some((h, r)) => (h, Some(r.trim()).filter(|r| !r.is_empty())),
                None => (rest, None),
            };
            if raw_handle.is_empty() {
                return Err(ParseError::MissingHandle { line });
            }

            let handle = Handle::parse(raw_handle, default_platform);
            if handle.platform.is_empty() || handle.name.is_empty() {
                return Err(ParseError::MalformedHandle {
                    line,
                    raw: raw_handle.to_string(),
                });
            }

            entries.push(Entry {
                handle,
                standing,
                reason: reason.map(str::to_string),
                line,
            });
        }

        Ok(Self { entries })
    }

    /// Looks up a handle. If the file lists someone as both vouched and
    /// denounced, the denouncement wins: a stale vouch shouldn't hide a ban.
    pub fn lookup(&self, handle: &Handle) -> Option<&Entry> {
        let mut found = None;
        for entry in self.entries.iter().filter(|e| &e.handle == handle) {
            if entry.standing == Standing::Denounced {
                return Some(entry);
            }
            found.get_or_insert(entry);
        }
        found
    }

    pub fn with_standing(&self, standing: Standing) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.standing == standing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# header comment

alice
codeberg:Bob
-mallory
-github:eve spammed 40 repos in a day
  carol
";

    #[test]
    fn parses_every_form_in_the_spec() {
        let list = VouchList::parse(SAMPLE, "github").unwrap();
        let got: Vec<_> = list
            .entries
            .iter()
            .map(|e| (e.handle.to_string(), e.standing, e.reason.as_deref()))
            .collect();

        assert_eq!(
            got,
            vec![
                ("github:alice".into(), Standing::Vouched, None),
                ("codeberg:bob".into(), Standing::Vouched, None),
                ("github:mallory".into(), Standing::Denounced, None),
                (
                    "github:eve".into(),
                    Standing::Denounced,
                    Some("spammed 40 repos in a day")
                ),
                ("github:carol".into(), Standing::Vouched, None),
            ]
        );
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let list = VouchList::parse(SAMPLE, "github").unwrap();
        let entry = list.lookup(&Handle::parse("@Alice", "github")).unwrap();
        assert_eq!(entry.line, 3);
        assert!(list.lookup(&Handle::parse("alice", "codeberg")).is_none());
    }

    #[test]
    fn denouncement_beats_an_earlier_vouch() {
        let list = VouchList::parse(
            "dave\n-dave changed their ways, not for the better\n",
            "github",
        )
        .unwrap();
        let entry = list.lookup(&Handle::parse("dave", "github")).unwrap();
        assert_eq!(entry.standing, Standing::Denounced);
    }

    #[test]
    fn rejects_a_bare_dash() {
        let err = VouchList::parse("alice\n-\n", "github").unwrap_err();
        assert_eq!(err, ParseError::MissingHandle { line: 2 });
    }

    #[test]
    fn rejects_an_empty_platform() {
        let err = VouchList::parse(":alice\n", "github").unwrap_err();
        assert!(matches!(err, ParseError::MalformedHandle { line: 1, .. }));
    }
}
