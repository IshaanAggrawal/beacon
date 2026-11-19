//! Sanity checks against Ghostty's real vouch list (see data/ghostty/SOURCE.md).
//! If the fixture is refreshed these counts will move; update them alongside it.

use beacon::ledger::vouch_td::{Handle, Standing, VouchList};

const VOUCHED: &str = include_str!("../data/ghostty/VOUCHED.td");

fn ghostty() -> VouchList {
    VouchList::parse(VOUCHED, "github").expect("Ghostty's list should parse cleanly")
}

#[test]
fn snapshot_counts() {
    let list = ghostty();
    assert_eq!(list.with_standing(Standing::Vouched).count(), 397);
    assert_eq!(list.with_standing(Standing::Denounced).count(), 28);
}

#[test]
fn every_entry_is_github_and_unique() {
    let list = ghostty();
    assert!(list.entries.iter().all(|e| e.handle.platform == "github"));

    let mut handles: Vec<_> = list.entries.iter().map(|e| &e.handle).collect();
    handles.sort();
    handles.dedup();
    assert_eq!(handles.len(), list.entries.len());
}

#[test]
fn most_denouncements_carry_a_reason() {
    let list = ghostty();
    let with_reason = list
        .with_standing(Standing::Denounced)
        .filter(|e| e.reason.is_some())
        .count();
    assert_eq!(with_reason, 19);
}

#[test]
fn known_maintainer_is_vouched() {
    let list = ghostty();
    let entry = list.lookup(&Handle::parse("mitchellh", "github")).unwrap();
    assert_eq!(entry.standing, Standing::Vouched);
}
