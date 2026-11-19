# Architecture

beacon is one Rust binary. The library is split by responsibility, and the CLI,
benchmark and web demo are thin layers over it.

```
src/ledger/    what a project writes down: policy, signed ledger, Vouch import, peer sync
src/graph/     what a project concludes: trust in other projects, evidence about users
src/report.rs  plain-English rendering of graph results
src/main.rs    CLI
```

This document records the decisions that weren't obvious, and what we gave up
for each.

## Ledger

Each project keeps two files in its own repo:

- `.trust/policy.toml` — the project id, maintainer public keys, where peers
  publish their data, and tuning knobs.
- `.trust/ledger.jsonl` — one attestation per line: `vouch`,
  `provisional_vouch`, `denounce`, `revoke`, `appeal`, `rehabilitate` or
  `trust_peer`.

Every entry is signed with ed25519 by a key listed in the policy, and names
the hash of the entry before it. The link is set *before* signing, so
deleting an entry and patching the next link needs a maintainer key; an edit
anywhere breaks the chain from that point on.

**Trade-off: keys are rooted in the repo.** Whoever can change a project's
`policy.toml` can add a key. That's the same trust boundary Vouch has today
(write access to the repo). What signatures add is per-maintainer
accountability and tamper-evidence for copies of the ledger fetched from
mirrors, which matters now that Ghostty is leaving GitHub.

**Trade-off: peer weights live in the ledger, not the config.** How much we
trust a peer is a signed, dated `trust_peer` attestation, so peer
relationships have history and can be checked. Only the fetch URL is plain
config.

### Importing Vouch files

`VOUCHED.td` has no dates or authors. Vouch's bot records both in its commit
messages, so the importer reads the file's git history: 381 of Ghostty's 425
entries get a real date and the maintainer (or CODEOWNERS sync) who made the
call. The rest are marked undated and shown as "date not recorded" rather
than with a made-up age. Imported entries stay unsigned and labelled as
imports; a policy can refuse them with `accept_imported = false`.

Quirks handled because the real data has them: handles are lowercased (the
file is lowercase, commit messages aren't), and a "reason" that only repeats
the handle is dropped (`!vouch alice` sometimes stored `alice` as the reason).

## Graph

Nothing is global. Every question is asked from a root project.

### Who can vouch

Following Vouch, only projects vouch; users never vouch for other users. So
the Sybil attack to worry about isn't fake *accounts* vouching for each other,
it's cheap fake *projects* vouching for each other and for spam accounts.

### Trust in projects

The root's trust in a project is its best path: multiply the `trust_peer`
weights along the path, multiply by `hop_decay` (0.5) for each hop past a
direct peer, and stop at `max_hops` (4). Paths are relaxed one hop at a time,
so the hop limit is exact rather than an approximation.

### Evidence about a user: max-flow, not counting

"How many projects denounce X" is the wrong question: ten fake projects would
count ten times. Instead, the evidence for or against a user is a **max-flow**
from the root:

- each project is a node that can pass on at most as much as the root trusts it,
- edges follow `trust_peer` links,
- a project that vouched for (or denounced) the user connects to the sink, with
  capacity equal to its trust times the statement's strength.

A ring of fake projects behind one real project shares that project's
capacity, so forty fakes add nothing over one. The test
`a_sybil_ring_is_capped_by_its_one_real_link` checks exactly this. Truly
independent sources do add up, which is what corroboration means.

The flow is decomposed into paths, so every number comes with "who said it,
and through whom". That's also what makes the counterfactual "would this
change without project X?" cheap: `support_excluding`.

**Trade-off: we wrote our own max-flow.** It's about 130 lines of Edmonds–Karp over
real-valued capacities. petgraph would have been a dependency for storage we
don't need, and we need the flow internals for path decomposition anyway. The
graphs are dozens of projects, so the algorithm's complexity doesn't matter.

### Backward blame

A peer makes a *bad call*, from the root's point of view, when it vouches for
someone the root has quarantined (denounced, or caught in a campaign), or
denounces someone the root vouches for. Each bad call multiplies the root's
trust in that peer by `blame_factor` (0.7), fading with a 90-day half-life.
One mistake dents a peer (−30%); three cut it to a third; ten make it
irrelevant.

**Trade-off: count, don't divide.** We considered blaming by the *share* of a
peer's vouches that went bad. That's fairer to big, busy projects, but it can
be gamed: a hostile project mass-vouches for harmless accounts to dilute its
bad ones. Counting bad calls can't be diluted. The cost is that a large,
honest project that occasionally gets fooled loses trust faster than its
error rate alone would justify; the half-life is what lets it recover.

**Trade-off: disagreeing with the root counts against a peer.** Denouncing
someone the root vouches for is treated as a bad call. That's what lets the
network shrug off a hostile project that starts denouncing honest
maintainers. But it also means a peer that is right and the root wrong gets
penalized in the root's view. That's acceptable because trust here is
subjective by design: each project only changes its own view.

### Time

- Other projects' denouncements fade with a 180-day half-life. Ghostty's list
  is nine months old and already has entries like "No HITL"; a ban shouldn't
  last forever by default.
- The root's own denouncements don't fade, since the root can lift them
  itself with `rehabilitate`.
- `appeal` records that someone asked to be reconsidered, and is shown
  alongside the denouncement. It doesn't change any numbers by itself; the
  project still has to decide. Ghostty already had to hand-fix a `!denounce`
  that hit the wrong person, so a clean way back matters.
- Provisional vouches expire and only count for the project that made them.
  A maintainer can reward a good first PR without the account counting for
  anything elsewhere.

If a project's ledger both vouches and denounces someone, the later statement
wins. If a raw Vouch file lists both, the denouncement wins: a stale vouch
shouldn't hide a ban.
