//! Deterministic plain-English explanations. This is what maintainers see
//! when no LLM is configured, and what the LLM falls back to when it fails.

use std::fmt::Write;

use super::evidence::{Evidence, Source};
use super::verdict::Tier;
use crate::report::pct;

/// Like `pct`, but doesn't round a real, tiny contribution down to "0%".
fn share(x: f64) -> String {
    if x > 0.0 && x < 0.005 {
        "under 1%".into()
    } else {
        pct(x)
    }
}

fn who(project: &str, root: &str) -> String {
    if project == root {
        "you".into()
    } else {
        project.to_string()
    }
}

fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

fn named_sources(e: &Evidence, sources: &[Source]) -> String {
    let names: Vec<String> = sources
        .iter()
        .take(3)
        .map(|s| who(&s.project, &e.root))
        .collect();
    match sources.len().saturating_sub(3) {
        0 => list(&names),
        more => format!("{}, and {more} more", names.join(", ")),
    }
}

fn counterfactual_sentence(e: &Evidence) -> Option<String> {
    let names: Vec<String> = e
        .counterfactual
        .flips
        .iter()
        .map(|f| who(&f.without, &e.root))
        .collect();
    let first = e.counterfactual.flips.first()?;
    let becomes = first.becomes.label().to_lowercase();
    Some(match (e.tier, names.as_slice()) {
        (Tier::Quarantined, [one]) => {
            format!("Flagged by a single source ({one}) — review before blocking.")
        }
        (Tier::Quarantined, _) => format!(
            "Removing any one of {} would lift the quarantine, so review before blocking.",
            list(&names)
        ),
        (Tier::Trusted, [one]) => {
            format!("That trust rests on {one} alone; without it they'd be a {becomes}.")
        }
        (Tier::Trusted, _) => format!(
            "That trust leans on each of {}; without any one of them they'd be a {becomes}.",
            list(&names)
        ),
        (Tier::CleanNewcomer, _) => {
            format!(
                "Without {}, the answer would be \"{becomes}\".",
                list(&names)
            )
        }
    })
}

/// Two sentences: the decision and its main reason, then what to watch for.
pub fn summary(e: &Evidence) -> String {
    let subject = &e.subject;
    let first = match e.tier {
        Tier::Quarantined => match (&e.campaign, e.denouncements.total >= 1.0 - 1e-9) {
            (_, true) => {
                let reason = e
                    .denouncements
                    .sources
                    .iter()
                    .find_map(|s| s.reason.clone())
                    .map(|r| format!(" (\"{r}\")"))
                    .unwrap_or_default();
                format!(
                    "Quarantine {subject}: denounced by {}, together worth {} of your own word{reason}.",
                    named_sources(e, &e.denouncements.sources),
                    pct(e.denouncements.total)
                )
            }
            (Some(c), false) => format!(
                "Quarantine {subject}: this PR is part of a coordinated wave of {} near-identical PRs from {} accounts across {} repos within {} hours.",
                c.prs,
                c.accounts,
                c.repos,
                c.hours.max(1)
            ),
            (None, false) => format!("Quarantine {subject}."),
        },
        Tier::Trusted => format!(
            "Fast-track {subject}: vouched for by {}, worth {} of your own word.",
            named_sources(e, &e.vouches.sources),
            pct(e.vouches.total)
        ),
        Tier::CleanNewcomer if e.denouncements.sources.is_empty() => format!(
            "{subject} is new to you: nobody you trust has vouched for or denounced them, and this PR doesn't match any campaign."
        ),
        Tier::CleanNewcomer => format!(
            "{subject} is mostly unknown to you, but {} denounced them, worth only {} of your own word, so they aren't blocked.",
            named_sources(e, &e.denouncements.sources),
            pct(e.denouncements.total)
        ),
    };

    let second = counterfactual_sentence(e).unwrap_or_else(|| match e.tier {
        Tier::Quarantined if e.campaign.is_some() && e.vouches.sources.is_empty() => {
            "Nobody you trust vouches for them, so there's nothing to outweigh the campaign match.".into()
        }
        Tier::Quarantined if e.campaign.is_some() => format!(
            "Vouches from {} weren't strong enough to outweigh it, and those projects lose some of your trust for it.",
            named_sources(e, &e.vouches.sources)
        ),
        Tier::Quarantined => "No single project's removal would change this.".into(),
        Tier::Trusted if e.vouches.sources.iter().all(|s| s.provisional) => {
            "It's a provisional vouch, so it only counts here and expires.".into()
        }
        Tier::Trusted => "No single project's removal would change this.".into(),
        Tier::CleanNewcomer if e.denouncements.sources.is_empty() => {
            "Review it like any first contribution; being new is not a red flag.".into()
        }
        Tier::CleanNewcomer => "Read the reason and review this PR before merging.".into(),
    });

    format!("{first} {second}")
}

/// The full evidence, for a terminal.
pub fn card(e: &Evidence) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} — {} (as seen by {})",
        e.subject,
        e.tier.label(),
        e.root
    );

    let side = |out: &mut String, title: &str, sources: &[Source], total: f64| {
        if sources.is_empty() {
            let _ = writeln!(out, "  {title}: none");
            return;
        }
        let _ = writeln!(out, "  {title}: worth {} of your own word", share(total));
        for s in sources {
            let mut line = format!(
                "    • {} counts {}",
                who(&s.project, &e.root),
                share(s.weight)
            );
            if s.path.len() > 2 {
                let _ = write!(
                    line,
                    ", through {}",
                    s.path[1..s.path.len() - 1].join(" → ")
                );
            }
            match s.age_days {
                Some(0) => line.push_str(", today"),
                Some(d) => {
                    let _ = write!(line, ", {d} days ago");
                }
                None => line.push_str(", date not recorded"),
            }
            if let Some(a) = &s.actor {
                let _ = write!(line, " by {a}");
            }
            if s.strength < 0.99 {
                let _ = write!(line, " (faded to {} with age)", pct(s.strength));
            }
            if s.provisional {
                line.push_str(" (provisional)");
            }
            if s.under_appeal {
                line.push_str(" (under appeal)");
            }
            let _ = writeln!(out, "{line}");
            if let Some(r) = &s.reason {
                let _ = writeln!(out, "      \"{r}\"");
            }
        }
    };
    side(&mut out, "Vouched for", &e.vouches.sources, e.vouches.total);
    side(
        &mut out,
        "Denounced",
        &e.denouncements.sources,
        e.denouncements.total,
    );

    match &e.campaign {
        Some(c) => {
            let _ = writeln!(
                out,
                "  Campaign: {} PRs from {} accounts in {} repos within {} hours ({} matched on wording, {} on structure)",
                c.prs,
                c.accounts,
                c.repos,
                c.hours.max(1),
                c.text_links,
                c.layout_links
            );
        }
        None => {
            let _ = writeln!(out, "  Campaign: no match");
        }
    }

    let cf = &e.counterfactual;
    let _ = match (cf.checked, cf.flips.is_empty()) {
        (0, _) => writeln!(out, "  Single-source check: nothing to check"),
        (n, true) => writeln!(
            out,
            "  Single-source check: removing any one of {n} contributing projects doesn't change this ({} µs)",
            cf.micros
        ),
        (_, false) => writeln!(
            out,
            "  Single-source check: depends on {} ({} µs)",
            list(
                &cf.flips
                    .iter()
                    .map(|f| who(&f.without, &e.root))
                    .collect::<Vec<_>>()
            ),
            cf.micros
        ),
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::evidence::{CampaignEvidence, Counterfactual, Flip, Side};

    fn evidence(tier: Tier) -> Evidence {
        Evidence {
            subject: "github:x".into(),
            root: "ghostty".into(),
            tier,
            vouches: Side {
                total: 0.0,
                sources: vec![],
            },
            denouncements: Side {
                total: 0.0,
                sources: vec![],
            },
            campaign: None,
            counterfactual: Counterfactual {
                checked: 0,
                flips: vec![],
                micros: 3,
            },
        }
    }

    fn source(project: &str, weight: f64, reason: Option<&str>) -> Source {
        Source {
            project: project.into(),
            weight,
            path: vec!["ghostty".into(), project.into()],
            reason: reason.map(str::to_string),
            age_days: Some(3),
            strength: 1.0,
            provisional: false,
            under_appeal: false,
            actor: None,
        }
    }

    #[test]
    fn newcomers_are_told_being_new_is_fine() {
        let s = summary(&evidence(Tier::CleanNewcomer));
        assert!(
            s.contains("new to you") && s.contains("not a red flag"),
            "{s}"
        );
    }

    #[test]
    fn campaign_quarantine_explains_the_wave() {
        let mut e = evidence(Tier::Quarantined);
        e.campaign = Some(CampaignEvidence {
            prs: 12,
            accounts: 9,
            repos: 7,
            hours: 20,
            text_links: 0,
            layout_links: 30,
            young_share: 0.3,
        });
        let s = summary(&e);
        assert!(
            s.contains("12 near-identical PRs from 9 accounts across 7 repos within 20 hours"),
            "{s}"
        );
        assert!(card(&e).contains("30 on structure"));
    }

    #[test]
    fn single_source_quarantine_says_review_first() {
        let mut e = evidence(Tier::Quarantined);
        e.denouncements = Side {
            total: 1.0,
            sources: vec![source("zig", 1.0, Some("spam"))],
        };
        e.counterfactual = Counterfactual {
            checked: 1,
            flips: vec![Flip {
                without: "zig".into(),
                becomes: Tier::CleanNewcomer,
            }],
            micros: 4,
        };
        let s = summary(&e);
        assert!(
            s.contains("denounced by zig") && s.contains("(\"spam\")"),
            "{s}"
        );
        assert!(
            s.contains("Flagged by a single source (zig) — review before blocking."),
            "{s}"
        );
        assert_eq!(s.matches(". ").count() + 1, 2, "two sentences: {s}");
    }

    #[test]
    fn tiny_but_real_contributions_are_not_shown_as_zero() {
        let mut e = evidence(Tier::CleanNewcomer);
        e.denouncements = Side {
            total: 0.001,
            sources: vec![source("mirror-00", 0.001, None)],
        };
        assert!(card(&e).contains("mirror-00 counts under 1%"));
    }

    #[test]
    fn card_shows_paths_reasons_and_undated_entries() {
        let mut e = evidence(Tier::Trusted);
        let mut s = source("bun", 0.6, None);
        s.path = vec!["ghostty".into(), "zig".into(), "bun".into()];
        s.age_days = None;
        e.vouches = Side {
            total: 0.6,
            sources: vec![s],
        };
        let c = card(&e);
        assert!(
            c.contains("through zig") && c.contains("date not recorded"),
            "{c}"
        );
    }
}
