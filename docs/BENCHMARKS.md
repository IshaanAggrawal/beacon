# Benchmarks

Two questions:

1. What does real spam look like? We measured it before building a detector.
2. Does beacon catch more coordinated spam than a flat Vouch list *without*
   blocking more honest people? We replay a simulated federation through both.

Everything here can be reproduced:

```sh
python scripts/spam_shapes.py data/research/spam_shapes.json   # needs an authenticated gh CLI
cargo run --release -- bench --seed 1 --seeds 5
cargo run --release -- bench --seed 1 --seeds 3 --jitter 0.5
```

## 1. What real spam looks like

We sampled public pull requests that maintainers labelled as spam, through
GitHub's read-only search API, and kept **aggregate statistics only**: no PR
text, usernames or repository names. Raw output:
[`data/research/spam_shapes.json`](../data/research/spam_shapes.json).

| | Hacktoberfest 2020 `spam` | 2025–26 `slop` | `ai-slop` |
|---|---:|---:|---:|
| PRs seen | 1,000 | 1,000 | 627 |
| Documentation-only PRs | 36% | 27% | 13% |
| README-only PRs | 18% | 16% | 2% |
| Median files changed | 1 | 2 | 3 |
| Median lines changed | 13 | 85 | 75 |
| 90th percentile lines changed | 126 | 1,948 | 1,424 |
| Accounts under 30 days old | 19% | 40% | 0.8% |
| Median account age (days) | 388 | 586 | 1,509 |
| PRs from authors with several labelled PRs | 34% | 71% | 57% |
| Repos per repeat author (median / p90) | 1 / 2 | 1 / 2 | 1 / 2 |
| Hours between one author's PRs (median) | 0.3 | 1.6 | 1.1 |

What we took from it:

- **Account age is a weak spam signal.** AI-era slop comes from accounts four
  years old on average. beacon never uses age to decide anything, and the
  simulator draws spam accounts from a realistic age mix (30% under a month).
- **Classic spam is tiny and documentation-heavy.** That's what copy-paste and
  layout fingerprints are built for.
- **Today's AI slop is mostly individual, established accounts sending large
  code changes, repeatedly, to the same repo.** It isn't a cross-repo wave.
  The campaign detector won't catch it, and by design it shouldn't try to judge
  content as "AI-looking". What catches it is one project denouncing it and
  that denouncement reaching projects that trust it. The simulator's "repeat
  offenders" follow that shape.
- **Bursts are short.** One author's PRs are typically minutes to hours apart;
  the 90th percentile is two to eighteen days. A 72-hour window covers most of
  a wave.

Caveat: we only see PRs in repos that bother to label spam, and a repo labels
its own PRs. That undercounts how far one account spreads across repos.

## 2. Flat Vouch vs beacon

### The simulated federation

`src/sim/world.rs`, seeded, 14 days:

- 30 honest projects. One is Ghostty with its **real list** (397 vouches, 28
  denouncements, real dates), with handles pseudonymized because the PR traffic
  is invented. The rest have synthetic vouch lists. Each trusts two to five
  others.
- About 2,400 established contributors (~1,500 PRs) and 200 honest newcomers
  (~250 PRs) with no vouches anywhere. 40% of newcomer PRs are tiny docs fixes,
  the same shape as classic spam.
- An **honest docs sprint**: 8 newcomers add near-identical example sections to
  two repos within 48 hours. It looks coordinated and isn't.
- **Copy-paste campaign**: 25 accounts, identical README additions, 1–3 repos
  each, within 24 hours.
- **Paraphrased campaign**: 35 accounts, same structure but different words in
  every PR, within 48 hours.
- **Repeat offenders**: 68 accounts already denounced by one to three projects
  (Ghostty's real 28 included), sending large multi-file PRs, mostly to one repo.
- **Hostile project**: trusted by Ghostty (70%) and two others. It vouches for
  20 campaign accounts and denounces 17 honest people, 8 of them Ghostty's own
  contributors.
- **Sybil ring**: 40 fake projects that trust each other, vouch for every
  campaign account and denounce the same 17 people. They're reachable only
  through one careless real project.

Every PR is judged by the project receiving it, in time order, with only what
was known at that moment.

### Results (seed 1)

1,925 PRs: 1,512 regular, 258 from newcomers, 115 campaign spam, 40 from repeat offenders.

| System | Campaign spam caught | Repeat offenders caught | Newcomer PRs blocked | Regular PRs blocked | Honest people blocked | Smear victims blocked | Precision | Hours to first block (copy-paste / paraphrased) |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Flat Vouch, require vouch | 100.0% | 100.0% | **100.0%** | 29.5% | 553 | 13 / 17 | 18.0% | 0.0 / 0.0 |
| Flat Vouch, denounce only | 0.0% | 0.0% | 0.0% | 0.0% | 0 | 0 / 17 | — | never / never |
| Flat Vouch + peers' denounce lists | 0.0% | 40.0% | 2.3% | 1.3% | 12 | 12 / 17 | 39.0% | never / never |
| **beacon** | **93.9%** | 2.5% | **0.0%** | **0.0%** | **0** | **0 / 17** | **100.0%** | 2.3 / 2.6 |

beacon also lets through, with a "review first" warning, 80% of repeat-offender
PRs, 4.7% of newcomer PRs and 5.8% of regular PRs.

Across seeds 1–5, beacon caught 93.9–95.0% of campaign spam and blocked no
honest PRs on any seed. Its first block in each campaign came 1.0–4.6 hours
after the campaign started.

### Reading the table

- **Require-vouch** stops everything bad by stopping almost everyone: every
  newcomer and 30% of established contributors (people not on *this*
  project's list). That's the closed-shop worry people raised about Vouch.
- **Denounce-only** is harmless and useless against new accounts.
- **Adopting peers' denounce lists** is how Ghostty's file suggests sharing.
  It catches 40% of repeat offenders, but it also hands the hostile project and
  the Sybil ring a ban hammer: 12 of the 17 people they smear get blocked, and
  most of what it blocks is honest.
- **beacon** catches the campaigns without anyone having denounced the
  accounts, and none of the smears land. The hostile project loses Ghostty's
  trust the moment it denounces Ghostty's own contributors and vouches for
  flagged accounts. The Sybil ring's 40 projects add up to one careless link's
  worth of capacity.

### Where beacon is weaker, honestly

- **Repeat offenders denounced by only one peer aren't auto-blocked** (2.5–7.5%
  caught). One project's word isn't enough to quarantine someone, which is
  exactly what stops smears. Instead most of their PRs (72–95%) get a "review
  first" warning. The same rule means about 5% of honest newcomer PRs carry
  that warning, mostly people the hostile project smeared.
- **Restructured paraphrases get through more.** If an attacker's LLM varies
  the structure as well as the words (`--jitter`), recall drops:

  | Structural variation | Campaign spam caught (3 seeds) | Honest PRs blocked |
  |---|---:|---:|
  | none | 94–95% | 0 |
  | 20% | 79–82% | 0 |
  | 50% | 63–72% | 0 |
  | 80% | 71–80% | 1 newcomer PR |

  Variation only affects the paraphrased campaign; copy-paste detection is
  unchanged. Even at the highest setting it fails safe: precision stays above
  98%.
- **Layout matching only covers documentation-only PRs.** Short code diffs
  share layouts for innocent reasons, and in our first run that produced false
  matches on 14 honest PRs. Real data says 13–36% of spam is docs-only, so
  code-shaped campaigns rely on near-identical text alone.
- **This is a simulation we wrote alongside the detector.** It shows the
  mechanisms work as designed against the attacks we modelled, and the shape of
  the trade-offs. It is not a measurement of real-world accuracy.

### How the detector got here

The first version used layout matching for every PR. It blocked 19.6% of
regular PRs, because every long diff contains every kind of line. Limiting
layout to additions of 40 lines or fewer brought that to 0.9%. The remaining 14
false blocks all came from layout links between short *code* PRs in popular
repos, so layout matching is now limited to documentation-only PRs, and false
blocks went to zero. `beacon bench` prints every honest PR it blocks along with
the cluster evidence, which is how both problems were found.
