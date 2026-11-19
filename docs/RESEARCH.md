# Research notes: Vouch, Ghostty, and what a trust *network* adds

These notes cover what we learned from Mitchell Hashimoto's Vouch and Ghostty's real
list before writing any trust or detection code, and how we plan to measure whether
beacon is actually better. The fixture behind every number here is in
[`data/ghostty/`](../data/ghostty/SOURCE.md).

## How Vouch works

[Vouch](https://github.com/mitchellh/vouch) is a small Nushell tool plus a set of
GitHub Actions. A project keeps a `VOUCHED.td` file (or `.github/VOUCHED.td`):

```text
# comments start with #
alice
codeberg:bob
-mallory reason for the denouncement
```

- One handle per line. An optional `platform:` prefix names the forge.
- A leading `-` denounces. Anything after the handle is a free-text reason.
- `vouch check <user>` exits 0 (vouched), 1 (denounced) or 2 (unknown).
- Maintainers vouch or denounce by commenting `!vouch` / `!denounce` on an issue or
  discussion. A bot then commits the change.
- PR and issue checks can auto-close anything from a denounced user. With
  `--require-vouch` they also close anything from unknown users.
- Vouched users can't vouch for anyone else. Only maintainers can.

The README mentions a future "Trustdown" spec for projects to share lists, but it
isn't published. Today there is no federation: each list stands alone.

## What Ghostty's real list looks like

Snapshot taken 2026-11-19:

| | |
|---|---|
| Entries | 425 |
| Vouched | 397 |
| Denounced | 28 (19 with a reason) |
| Entries with a `platform:` prefix | 0 |
| Commits touching the file | 382, from 2026-02-15 to 2026-11-18 |
| Parsed bot actions | 290 vouch, 28 denounce, 4 unvouch |
| Maintainers who triggered actions | 10 (two of them did about 70%) |

Things we didn't expect, and what they change:

1. **The file forgets who vouched and when.** It's a flat list with no dates and no
   author. But the bot's commit messages record both ("Triggered by comment from
   @jcollie … Vouch: @orion0616"). The importer will read git history to recover a
   timestamp and a voucher for most entries. Without timestamps, any time decay
   would be guesswork.
2. **Denouncement reasons are about behavior, not prose style.** "No HITL",
   "meatproxying", "refusing to follow AI policy". These are judgments about how
   someone acted after being asked to change. That's consistent with our rule of not
   trying to detect AI writing: we treat a reason as evidence text for humans to
   read, never as a feature to classify.
3. **Not every note is a reason.** One vouched entry is `heddxh heddxh`. A note on a
   vouch is not a reason to distrust anyone, so we only show notes; we never
   interpret them.
4. **Some vouches are bulk and automatic.** Weekly "Sync CODEOWNERS" commits vouch
   every code owner at once. That's a different, stronger signal (commit access)
   than a single `!vouch` on a first PR. We keep the origin on each imported
   attestation so policy can weigh them differently.
5. **Handles are case-insensitive.** The file is lowercase, commit messages aren't
   (`@MrMage` vs `mrmage`). We normalize to lowercase everywhere.
6. **Unvouching happens.** Four entries were removed after the fact, so revocation
   is a real operation, not an edge case.
7. **Ghostty is leaving GitHub.** GitHub will stay as a read-only mirror. Identities
   must carry their forge (`github:alice`) and nothing can assume GitHub is the
   only source of truth.

The file's own header says the denounce list exists so "other projects can slurp in
our list of denounced users if they trust us". That's transitive trust, done by
hand, with all-or-nothing weight. beacon's job is to make that safe.

## Where a flat list falls short

| Situation | Flat Vouch | What beacon does instead |
|---|---|---|
| A newcomer is vouched by a project you trust | Unknown to you | Trust flows along signed project-to-project edges, decaying per hop |
| Someone edits the list | Anyone with write access; no signature | Every attestation is signed by a maintainer key the project lists |
| A ban from February | Permanent | Imported denouncements fade over time, and a signed appeal plus rehabilitation can lift them |
| You slurp another project's denounce list | That project can ban anyone in your repo | One source alone can only flag, not block; quarantine needs corroboration |
| A trusted project starts vouching for spammers | Nothing changes | Backward blame cuts your trust in that project automatically |
| A ring of fresh repos vouch for each other | Not applicable today, but would be total if lists were merged | Max-flow caps: the whole ring gets no more trust than its single link into your graph |
| 40 new accounts open near-identical PRs in 40 repos | 40 separate unknowns | Cross-project fingerprints cluster them into one campaign |
| `--require-vouch` | Blocks every newcomer | "Clean newcomer" lane: unknown with no risk signals is never auto-blocked |

## Benchmark design

The benchmark has to be reproducible from a seed, include real data, and answer one
question: does beacon catch more coordinated spam than a flat list *without*
blocking more honest newcomers?

**World** (fixed seed, generated by `src/sim`):

- ~30 projects. Ghostty is one of them, with its real list. The others get
  synthetic lists whose sizes and vouch/denounce ratios follow Ghostty's (about 7%
  denounced).
- ~2,000 established contributors spread across projects.
- 200 honest newcomers: no vouches anywhere, ordinary PRs.
- A spam campaign: 60 fresh accounts open paraphrased PRs across 25 repos over 48
  hours. Text varies from PR to PR; the shape of the diff stays similar.
- A Sybil ring: 40 freshly created projects that vouch for each other and for the
  campaign accounts. They're linked to the honest graph by a single edge.
- One hostile project that is genuinely trusted by a few others, vouches for the
  spammers, and denounces some honest maintainers.

**Systems compared:**

1. Flat Vouch, `--require-vouch`: only vouched users pass.
2. Flat Vouch, denounce-only: everyone passes unless denounced.
3. Flat Vouch + slurp: union of every project's denounce list.
4. beacon.

**Metrics:**

- Spam caught (recall) and spam precision.
- **Newcomer false-block rate.** The share of honest newcomers blocked. This is the
  number we care most about; we'd accept losing some recall to keep it near zero.
- Time from the campaign's first PR to detection.
- Hostile blast radius: honest users the hostile project manages to get blocked.

Before tuning campaign thresholds, PR #3 will sample public metadata of PRs that
maintainers labelled `spam` during past Hacktoberfest events (read-only search API).
We'll keep only aggregate shape statistics: files touched, churn, timing between
PRs. That checks whether our synthetic campaign looks like real ones. No PR text or
usernames are stored.
