//! Turns a Vouch file into attestations, using the file's git history to
//! recover when each decision was made and by whom.
//!
//! `VOUCHED.td` only says *who* is vouched or denounced. Vouch's bot writes
//! commit messages like
//!
//! ```text
//! Triggered by [comment](https://github.com/ghostty-org/ghostty/issues/1#...)
//! from @mitchellh.
//!
//! Vouch: @MrMage
//! ```
//!
//! which gives us a date and an actor for most entries.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::attestation::{Attestation, Kind, Origin};
use super::vouch_td::{Handle, Standing, VouchList};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Vouch,
    Denounce,
    Unvouch,
    /// Bulk vouch of everyone in CODEOWNERS.
    CodeownersSync,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEvent {
    pub date: DateTime<Utc>,
    pub actor: Option<String>,
    pub action: Action,
    pub subject: Handle,
}

#[derive(Deserialize)]
struct Commit {
    date: DateTime<Utc>,
    message: String,
}

/// Parses `history.jsonl` (one `{sha, date, message}` object per line).
/// Commits that don't follow the bot's format are skipped, not errors.
pub fn parse_history(jsonl: &str, forge: &str) -> Result<Vec<HistoryEvent>, serde_json::Error> {
    let mut events = Vec::new();
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let commit: Commit = serde_json::from_str(line)?;
        events.extend(events_in_message(&commit.message, commit.date, forge));
    }
    events.sort_by_key(|e| e.date);
    Ok(events)
}

fn events_in_message(message: &str, date: DateTime<Utc>, forge: &str) -> Vec<HistoryEvent> {
    let mut out = Vec::new();

    if message.starts_with("Sync CODEOWNERS") {
        for line in message.lines() {
            if let Some(name) = line.trim().strip_prefix("- @") {
                out.push(HistoryEvent {
                    date,
                    actor: None,
                    action: Action::CodeownersSync,
                    subject: Handle::new(forge, name.trim()),
                });
            }
        }
        return out;
    }

    // "from @jcollie." can be split across lines by the bot's wrapping.
    let flat = message.replace('\n', " ");
    let actor = flat.split("from @").nth(1).map(|rest| {
        rest.split(|c: char| c.is_whitespace() || c == '.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    });

    for line in message.lines() {
        let Some((verb, rest)) = line.split_once(':') else {
            continue;
        };
        let action = match verb.trim() {
            "Vouch" => Action::Vouch,
            "Denounce" => Action::Denounce,
            "Unvouch" => Action::Unvouch,
            _ => continue,
        };
        if let Some(name) = rest.split_whitespace().next() {
            out.push(HistoryEvent {
                date,
                actor: actor.clone().filter(|a| !a.is_empty()),
                action,
                subject: Handle::parse(name, forge),
            });
        }
    }
    out
}

/// Converts a Vouch list into unsigned attestations issued by `project`.
///
/// Each entry takes its date and actor from the latest matching history
/// event. Entries with no history (the initial import, hand edits) get
/// `fallback_date`.
pub fn import_vouch_list(
    list: &VouchList,
    history: &[HistoryEvent],
    project: &str,
    source: &str,
    fallback_date: DateTime<Utc>,
) -> Vec<Attestation> {
    let mut latest: HashMap<(&Handle, bool), &HistoryEvent> = HashMap::new();
    for event in history {
        let is_denounce = match event.action {
            Action::Denounce => true,
            Action::Vouch | Action::CodeownersSync => false,
            Action::Unvouch => continue,
        };
        latest.insert((&event.subject, is_denounce), event);
    }

    let mut out: Vec<Attestation> = list
        .entries
        .iter()
        .map(|entry| {
            let is_denounce = entry.standing == Standing::Denounced;
            let event = latest.get(&(&entry.handle, is_denounce));
            let kind = if is_denounce {
                Kind::Denounce
            } else {
                Kind::Vouch
            };

            let mut a = Attestation::new(
                kind,
                project,
                &entry.handle.to_string(),
                event.map_or(fallback_date, |e| e.date),
            );
            a.reason = entry
                .reason
                .clone()
                // `!vouch alice` sometimes stored "alice" as the reason too
                // (Ghostty fixed one such case by hand in #13050).
                .filter(|r| !r.eq_ignore_ascii_case(&entry.handle.name));
            a.origin = Origin::Imported {
                source: source.into(),
                actor: event.and_then(|e| match e.action {
                    Action::CodeownersSync => Some("CODEOWNERS".into()),
                    _ => e.actor.clone(),
                }),
            };
            a
        })
        .collect();

    // Ledgers are append-only in time order.
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then(a.subject.cmp(&b.subject))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::attestation::tests::at;

    fn commit(day: u32, message: &str) -> String {
        serde_json::json!({ "sha": "x", "date": at(day), "message": message }).to_string()
    }

    #[test]
    fn reads_bot_messages_including_wrapped_actor() {
        let jsonl = [
            commit(1, "Update VOUCHED list (#1)\n\nTriggered by [comment](u)\nfrom @mitchellh.\n\nVouch: @MrMage\n"),
            commit(2, "Update VOUCHED list (#2)\n\nTriggered by [discussion\ncomment](u) from\n@jcollie.\n\nDenounce: @spammer\n"),
            commit(3, "Sync CODEOWNERS vouch list (#3)\n\n## Added Users\n\n- @Atomk\n- @gpanders\n"),
            commit(4, "Use mitchellh/vouch"),
        ]
        .join("\n");

        let events = parse_history(&jsonl, "github").unwrap();
        let got: Vec<_> = events
            .iter()
            .map(|e| (e.subject.to_string(), e.action, e.actor.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("github:mrmage".into(), Action::Vouch, Some("mitchellh")),
                ("github:spammer".into(), Action::Denounce, Some("jcollie")),
                ("github:atomk".into(), Action::CodeownersSync, None),
                ("github:gpanders".into(), Action::CodeownersSync, None),
            ]
        );
    }

    #[test]
    fn import_takes_date_and_actor_from_history() {
        let list =
            VouchList::parse("mrmage\nold-timer\n-spammer bad\nheddxh heddxh\n", "github").unwrap();
        let history = vec![
            HistoryEvent {
                date: at(10),
                actor: Some("mitchellh".into()),
                action: Action::Vouch,
                subject: Handle::new("github", "MrMage"),
            },
            HistoryEvent {
                date: at(20),
                actor: Some("jcollie".into()),
                action: Action::Denounce,
                subject: Handle::new("github", "spammer"),
            },
        ];

        let out = import_vouch_list(&list, &history, "ghostty", "VOUCHED.td", at(0));
        let find = |s: &str| out.iter().find(|a| a.subject == s).unwrap();

        let mrmage = find("github:mrmage");
        assert_eq!(mrmage.created_at, at(10));
        assert!(
            matches!(&mrmage.origin, Origin::Imported { actor: Some(a), .. } if a == "mitchellh")
        );

        let spammer = find("github:spammer");
        assert_eq!(spammer.kind, Kind::Denounce);
        assert_eq!(spammer.created_at, at(20));
        assert_eq!(spammer.reason.as_deref(), Some("bad"));

        assert_eq!(find("github:old-timer").created_at, at(0));
        assert_eq!(
            find("github:heddxh").reason,
            None,
            "handle echoed as reason is dropped"
        );
        assert!(out.windows(2).all(|w| w[0].created_at <= w[1].created_at));
    }
}
