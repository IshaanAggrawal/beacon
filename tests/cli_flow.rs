//! End-to-end: two projects, real git, real signatures, the actual binary.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_beacon");
const NOW: &str = "2026-11-19T00:00:00Z";

fn beacon(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN)
        // Never reach a real LLM from tests: no keys in the environment, and
        // run outside the repo so a developer's .env isn't picked up.
        .env_remove("GROQ_API_KEY")
        .env_remove("GROQ_MODEL")
        .current_dir(dir)
        .arg("--dir")
        .arg(dir)
        .args(["--at", NOW])
        .args(args)
        .output()
        .expect("binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (success, text) = beacon(dir, args);
    assert!(success, "beacon {args:?} failed:\n{text}");
    text
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn a_peer_vouch_reaches_us_and_a_tampered_peer_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let zig = tmp.path().join("zig");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&zig).unwrap();

    // A peer project with its own ledger, published as a git repo.
    ok(&zig, &["init", "zig", "--maintainer", "andrew"]);
    ok(&zig, &["vouch", "newcomer", "--reason", "great first PR"]);
    ok(
        &zig,
        &["denounce", "spammer", "--reason", "same PR in 30 repos"],
    );
    assert!(ok(&zig, &["verify"]).contains("chain intact"));
    git(&zig, &["init", "-q", "-b", "main"]);
    git(&zig, &["add", "."]);
    git(&zig, &["commit", "-q", "-m", "trust data"]);

    // Our project trusts it and syncs.
    ok(&home, &["init", "home", "--maintainer", "me"]);
    let zig_url = zig.to_string_lossy().to_string();
    ok(&home, &["trust", "zig", "0.8", "--url", &zig_url]);
    ok(&home, &["sync"]);

    let peers = ok(&home, &["peers"]);
    assert!(peers.contains("zig") && peers.contains("80%"), "{peers}");

    let why = ok(&home, &["why", "newcomer"]);
    assert!(why.contains("Vouched for by 1 project"), "{why}");
    assert!(why.contains("great first PR"), "{why}");

    let why = ok(&home, &["why", "spammer"]);
    assert!(why.contains("Denounced by 1 project"), "{why}");

    // One peer at 80% isn't enough to quarantine on its own.
    let verdict = ok(&home, &["verdict", "spammer", "--no-llm"]);
    assert!(verdict.contains("Clean newcomer"), "{verdict}");
    assert!(verdict.contains("same PR in 30 repos"), "{verdict}");
    assert!(verdict.contains("built-in template"), "{verdict}");
    let verdict = ok(&home, &["verdict", "newcomer", "--no-llm"]);
    assert!(verdict.contains("Fast-track github:newcomer"), "{verdict}");
    assert!(verdict.contains("rests on zig alone"), "{verdict}");

    // Someone rewrites the peer's ledger without its key.
    let ledger = zig.join(".trust/ledger.jsonl");
    let text = std::fs::read_to_string(&ledger).unwrap();
    std::fs::write(&ledger, text.replace("github:spammer", "github:innocent")).unwrap();
    git(&zig, &["commit", "-q", "-am", "oops"]);

    let (success, out) = beacon(&home, &["sync"]);
    assert!(!success, "sync should report the bad peer");
    assert!(out.contains("ignoring a peer"), "{out}");

    let why = ok(&home, &["why", "innocent"]);
    assert!(why.contains("Nobody you trust has denounced them"), "{why}");
}

#[test]
fn ghostty_import_keeps_who_and_when() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/ghostty");
    let vouched = data.join("VOUCHED.td").to_string_lossy().to_string();
    let history = data.join("history.jsonl").to_string_lossy().to_string();

    ok(dir, &["init", "ghostty", "--maintainer", "importer"]);
    let out = ok(dir, &["import", &vouched, "--history", &history]);
    assert!(out.contains("Imported 425 entries"), "{out}");

    let verify = ok(dir, &["verify"]);
    assert!(verify.contains("425 imported unsigned"), "{verify}");

    let why = ok(dir, &["why", "vectorpeak"]);
    assert!(why.contains("you"), "{why}");
    assert!(why.contains("cosplaying"), "{why}");
}
