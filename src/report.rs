//! Plain-English renderings of trust results. No raw scores without words.

use std::fmt::Write;

use crate::graph::decay::age_days;
use crate::graph::trust::Polarity;
use crate::graph::{BadCall, Support, TrustView};

/// "80%" reads better than "0.8" for people who aren't us.
pub fn pct(x: f64) -> String {
    format!("{:.0}%", (x * 100.0).clamp(0.0, 999.0))
}

pub fn peers(view: &TrustView) -> String {
    let mut out = String::new();
    let mut rows: Vec<_> = view
        .projects
        .iter()
        .filter(|(id, _)| **id != view.root)
        .collect();
    if rows.is_empty() {
        return format!("{} doesn't trust any other projects yet.\n", view.root);
    }
    rows.sort_by(|a, b| b.1.trust.total_cmp(&a.1.trust));

    let width = rows.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    let _ = writeln!(out, "How much {} trusts other projects:", view.root);
    for (id, p) in rows {
        let route = if p.hops == 1 {
            "direct peer".to_string()
        } else {
            format!("via {}", p.path[1..p.path.len() - 1].join(" → "))
        };
        let _ = writeln!(out, "  {id:<width$}  {:>4}  {route}", pct(p.trust));
        if let Some(blame) = &p.blame {
            let vouched: Vec<_> = blame
                .calls
                .iter()
                .filter_map(|(c, _)| match c {
                    BadCall::VouchedForQuarantined { subject } => Some(subject.as_str()),
                    _ => None,
                })
                .collect();
            let denounced: Vec<_> = blame
                .calls
                .iter()
                .filter_map(|(c, _)| match c {
                    BadCall::DenouncedRootsVouchee { subject } => Some(subject.as_str()),
                    _ => None,
                })
                .collect();
            let pad = " ".repeat(width + 4);
            let _ = writeln!(
                out,
                "  {pad}lowered to {} of normal because it",
                pct(blame.multiplier)
            );
            if !vouched.is_empty() {
                let _ = writeln!(
                    out,
                    "  {pad}  vouched for {} you've quarantined: {}",
                    people(vouched.len()),
                    sample(&vouched)
                );
            }
            if !denounced.is_empty() {
                let _ = writeln!(
                    out,
                    "  {pad}  denounced {} you vouch for: {}",
                    people(denounced.len()),
                    sample(&denounced)
                );
            }
        }
    }
    out
}

pub fn support(view: &TrustView, s: &Support) -> String {
    let mut out = String::new();
    let (verb, none) = match s.polarity {
        Polarity::For => ("Vouched for", "Nobody you trust has vouched for them."),
        Polarity::Against => ("Denounced", "Nobody you trust has denounced them."),
    };
    if s.is_empty() {
        let _ = writeln!(out, "  {none}");
        return out;
    }
    let _ = writeln!(
        out,
        "  {verb} by {} (combined weight {}, where your own word is 100%):",
        projects(s.statements.len()),
        pct(s.total)
    );
    for st in &s.statements {
        let who = if st.project == view.root {
            "you".to_string()
        } else {
            st.project.clone()
        };
        let when = if st.mark.dated {
            ago(age_days(st.mark.at, view.now))
        } else {
            "date not recorded".into()
        };
        let mut line = format!("    • {who}, {when}, counts {}", pct(st.amount));
        if let Some(actor) = &st.mark.actor {
            let _ = write!(line, " (by {actor})");
        }
        if let Some((path, _)) = st.paths.first().filter(|(p, _)| p.len() > 2) {
            let _ = write!(
                line,
                ", reached through {}",
                path[1..path.len() - 1].join(" → ")
            );
        }
        if st.strength < 0.99 {
            let _ = write!(line, "; faded to {} with age", pct(st.strength));
        }
        if st.provisional {
            line.push_str("; provisional, only counts here");
        }
        if st.under_appeal {
            line.push_str("; under appeal");
        }
        let _ = writeln!(out, "{line}");
        if let Some(reason) = &st.mark.reason {
            let _ = writeln!(out, "      reason: {reason}");
        }
    }
    out
}

fn ago(days: i64) -> String {
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=60 => format!("{days} days ago"),
        61..=729 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
    }
}

fn people(n: usize) -> String {
    if n == 1 {
        "1 person".into()
    } else {
        format!("{n} people")
    }
}

fn projects(n: usize) -> String {
    if n == 1 {
        "1 project".into()
    } else {
        format!("{n} projects")
    }
}

fn sample(items: &[&str]) -> String {
    const SHOW: usize = 3;
    if items.len() <= SHOW {
        items.join(", ")
    } else {
        format!(
            "{} and {} more",
            items[..SHOW].join(", "),
            items.len() - SHOW
        )
    }
}
