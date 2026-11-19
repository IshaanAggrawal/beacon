# Architecture

beacon is one Rust binary. The library is split by responsibility, and the CLI,
benchmark and web demo are thin layers over it.

```
src/ledger/    what a project writes down: policy, signed ledger, Vouch import, peer sync
src/graph/     what a project concludes: trust in other projects, evidence about users
src/campaign/  cross-project campaign detection from content-free fingerprints
src/triage/    combining trust and behavior into a verdict
src/sim/       seeded federation simulator and the benchmark
src/api/       local demo server: REST + server-sent events, UI embedded
src/report.rs  plain-English rendering of graph results
src/main.rs    CLI
ui/            the demo page: plain HTML, CSS and JS, no build step
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

## Campaign detection

Real spam-labelled PRs informed this design; see
[BENCHMARKS.md](BENCHMARKS.md#1-what-real-spam-looks-like).

### Fingerprints, not contents

Projects compare PRs without sharing them. A fingerprint keeps:

- a salted SHA-256 of the author handle,
- the repo and the hour,
- a 64-bit SimHash of the words (title, body, added lines),
- a 64-bit SimHash of the *layout*: the sequence of line kinds added
  (heading, bullet, link, code, prose...) as 1-, 2- and 3-grams,
- coarse diff geometry: a bitset of file extensions, log2 buckets for files,
  lines and hunks, a docs-only flag, an account-age bucket.

No text, no code, no handle. **Trade-off:** a salted hash of a public handle is
pseudonymous, not anonymous. Anyone holding the salt can hash a list of known
handles and match them. The salt is shared within a federation, so this
protects against a leaked fingerprint store, not against a member.

### Linking and clustering

Two PRs from *different* authors within 72 hours are linked when their text
hashes are within 6 bits (copy-paste), or when both are documentation-only, of
40 lines or fewer, with layout hashes within 4 bits and matching diff shapes
(paraphrase). Linked PRs form clusters. A cluster becomes a campaign at 4
distinct accounts **and** 4 distinct repos. One person repeating themselves, or
a docs sprint in two repos, never qualifies.

Matching is brute force over the window: exact, simple, and fast enough at
federation volumes (the benchmark judges ~1,900 PRs, including trust
computation, in under half a second).

**Trade-off: layout only for docs.** Layout survives paraphrasing, but short
code diffs share layouts for innocent reasons, and our first benchmark run
produced 14 false blocks that way. Restricting it to documentation-only PRs
removed them. The cost: code-shaped campaigns are only caught when their text
is nearly identical.

**What it deliberately doesn't do:** judge whether text "looks AI-written",
or treat new accounts as suspicious. Both would hurt honest newcomers, and
the real data shows account age barely separates spam from honest work anyway.

## Verdicts

`triage::assess` decides and gathers the evidence behind the decision.

- **Quarantined** when denouncement support reaches 1.0 (your own word, or
  independent projects you trust adding up to it), or when the author matched a
  campaign and vouches don't reach 1.0.
- **Trusted** when vouch support reaches 0.5.
- **Clean newcomer** otherwise. Never auto-blocked. Weaker denouncements come
  with the reason and a "review first" note, but don't block.

Identity (vouches) and behavior (campaigns) stay separate until this point.

### Single-source check

For every verdict, `assess` removes each contributing project in turn (every
project on any path the evidence took, not just the ones that spoke) and
decides again. If any one removal changes the answer, the card says so:
*"Flagged by a single source (zig) — review before blocking."* Because the
max-flow is small, this costs around 100–350 µs per verdict, so it runs on
every PR rather than only on request.

**Trade-off:** we check single removals, not pairs. Two colluding projects
that are each individually necessary still show up (both are listed). Two that
are each individually *redundant* but jointly decisive don't. Checking all pairs
is quadratic in contributors; single removals already catch the case that
matters most, a verdict hanging on one project's word.

### Explanations

Every verdict gets a two-sentence summary and an evidence card: who vouched or
denounced, through which projects, when, how much each counted, any fading or
appeal, campaign stats, and the single-source result. Weights are shown as a
share of the maintainer's own word, never as a bare score, and a real but tiny
contribution reads "under 1%" rather than "0%".

### The optional LLM

If `GROQ_API_KEY` and `GROQ_MODEL` are set (in the environment or `.env`), the
template's draft and a small set of pre-formatted facts (verdict, who vouched or
denounced and how much each counts, campaign details, which projects the
verdict hangs on) go to Groq's OpenAI-compatible API, which rewrites the draft
in plainer words. The model is configurable and never hardcoded.

The first version sent the raw evidence JSON and asked the model to reason from
it. Against a live Qwen model, that gave the wrong main reason for a campaign
catch ("quarantined because the sole vouch carries 0.0001%"), and claims about
single sources that weren't true. Giving it a correct draft to reword fixed all
six sample cases.

The LLM only rewords; it never decides. Its reply is discarded in favor of the
template if the request fails, times out (8 s) or is rate-limited, or if it
returns nothing usable. It's also discarded if the note **doesn't mention the
verdict, leaves out the campaign, leaves out or invents a single-source
warning, or doesn't name the main project**. Qwen-style `<think>` blocks are
stripped. The output says which one wrote the summary, and
why the template was used if an LLM was configured. The key is never logged:
`GroqConfig`'s `Debug` output leaves it out.

**Trade-off: what leaves the machine.** The facts contain handles, project
names and denouncement reasons, which are all public data. They contain no PR
content. Projects that don't want to send even that simply don't set a
key.

## Demo server

`beacon serve` runs the simulation behind a small axum server bound to
localhost. The page and its fonts are embedded with rust-embed, so the binary is
the whole demo; there's no build step and nothing loads from a CDN.

| Endpoint | What it returns |
|---|---|
| `GET /api/scenario` | The world's projects and populations, and the final scoreboard |
| `GET /api/stream` | Every PR in time order as server-sent events, at `speed` PRs per second |
| `GET /api/graph` | Trust from `root`, as known right after PR `upto` (campaign flags until then) |
| `GET /api/verdict` | PR `i`'s evidence, its card and its summary (LLM or template) |

All take `seed`, `hostile` and `sybil`. Scenarios are deterministic, so each is
computed once and cached. The attack toggles remove the hostile project or the
Sybil ring from the *same* world, so the PR stream is identical with and without
them and the comparison is clean.

The graph is drawn as a schematic: columns are hops from the root, nodes sit on
the page's 24px grid, and each node's square fills with the root's trust in it.
Picking a PR switches the graph to the receiving project's point of view and
lights up the paths its evidence took. The projects the verdict hangs on are
outlined with a dashed border.
