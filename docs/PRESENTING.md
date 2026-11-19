# Presenting the demo

A five-minute walkthrough of the live demo, plus what to check beforehand and
what to do if something goes wrong.

Live: <https://beacon-demo-8v3b.onrender.com>

## Before you start

- **Wake it up five minutes early.** The free instance sleeps after 15 minutes
  without visitors and takes about a minute to wake. Open the link, wait for
  "1925 pull requests over 14 days are ready", and leave the tab open. While
  it's open and on screen, the page checks in every 10 minutes so the
  instance stays awake.
- **Check the LLM.** Replay once, open any PR, and look under the summary. It
  should say "Written by qwen/…". If it says "built-in summary (the LLM rejected
  the API key)", open the service's **Environment** tab on Render and make sure
  `GROQ_API_KEY` and `GROQ_MODEL` have no quote marks around them. The demo
  works either way; the built-in summaries say the same thing.
- **Speed.** Seed 1 is simulated at startup in every toggle combination, so the
  toggles are instant. Another seed takes about 6 seconds the first time on the
  free CPU.
- **Screen.** Use a laptop-width window (1280px or more) so the graph,
  stream and scoreboard all fit. Zoom the browser to 110–125% for a projector.

## The walkthrough

### 1. The problem (30 s)

> Open-source projects are getting flooded with low-effort, AI-generated pull
> requests. Ghostty's answer is Vouch: a list of people allowed in, and people
> banned. It works, but a list is either a closed door for newcomers or an open
> one for spammers, and it can't see a spam wave that spans many projects.

### 2. The baseline (45 s)

Both attacks off. Press **16×**, then **Replay 14 days**.

> Thirty projects, fourteen days, about 1,900 pull requests. Ghostty's real
> vouch list is in here. Each project judges the PRs it receives, using only
> what it knows at that moment.

Point at the scoreboard when it finishes:

> "Require a vouch" stops all the spam by also blocking every single newcomer,
> 258 of them. beacon catches 94% of the coordinated spam and blocks zero
> newcomers.

### 3. A hostile project (60 s)

Turn on **Inject hostile project**, replay.

> quickforge is a real-looking project that Ghostty trusts at 70%. It starts
> vouching for spammers and denouncing Ghostty's own contributors.

Watch the big readout:

> Ghostty's trust in it is already down to 24%, because it denounced eight
> people Ghostty vouches for. As the accounts it vouched for get caught in
> campaigns, it falls under 1%. Nobody edited a config file. That's backward
> blame: vouching puts your reputation at stake.

On the scoreboard, point at **Smears that landed**:

> If projects just adopt each other's ban lists, which is how Vouch files are
> shared today, 12 of the 17 people it smeared get blocked. With beacon, none.

### 4. A Sybil ring (45 s)

Turn on **Inject Sybil ring**, replay.

> Forty fake projects that vouch for each other and for the spammers. They're
> connected to the real network through one careless project.

Point at the ring:

> Trust is computed as a flow. No project can pass on more than you trust it
> with, so forty fakes behind one link count exactly as much as one. Here
> they're worth nothing to Ghostty.

### 5. Why a PR was blocked (60 s)

Set **Show** to "quarantined" and click a `pp…` row (the reworded campaign).

> Every decision explains itself. This PR is part of a wave of near-identical
> PRs from dozens of accounts across 25 repos. They were reworded by an LLM so
> no two look alike, but the *structure* is the same, so they were matched
> on layout. No PR text is stored, only fingerprints.

Point at the summary:

> The summary is written by an open-weight model on Groq, but it only rewords
> evidence the Rust code already computed. If it says anything that doesn't
> match, we throw it away and use the built-in one.

The graph has switched to the receiving project's point of view, and the
paths behind the verdict are lit.

### 6. Single source (30 s)

Set **Show** to "let through with a warning" and open a row with a `!`.

> When a verdict rests on one project's word, beacon says so. Here one
> project denounced this person, but that alone isn't enough to block them, so
> they get through with a "review first" note. Being new is never a reason
> to block.

### 7. Close (20 s)

> No central authority: each project keeps a signed ledger in its own repo and
> decides whom to trust. We judge behavior and trust, never whether text
> "sounds like AI". And the limits are written down too: the benchmarks and
> threat model are in the repo.

## Likely questions

- **Is this real data?** Ghostty's vouch list is real, with its 397 vouches and
  28 denouncements, though handles are replaced in the demo. The spam shapes
  are calibrated on about 2,600 real spam-labelled PRs. The PR traffic itself
  is simulated.
- **What if many trusted projects gang up?** Then they can quarantine someone.
  Trust is subjective by design, and the evidence card shows exactly who did it.
- **Can spammers evade it?** Rewording alone doesn't help them. Rewording *and*
  restructuring drops detection to roughly 63–82%. It's in
  [BENCHMARKS.md](BENCHMARKS.md).
- **Why not detect AI-written text?** It's unreliable and punishes honest
  people, especially non-native English speakers. The real data also shows
  account age barely separates spam from honest work.
- **How fast is it?** About 1,900 PRs, with full trust computation, in under a
  second on a laptop. The free hosting CPU is a tenth of a core, so it takes
  about 6 seconds there. The single-source check takes about 100–350
  microseconds per verdict.

## If something goes wrong

- **Page is slow to load:** the instance is waking up. Give it a minute.
- **No internet:** run it locally with `cargo run --release -- serve` or
  `docker run -p 8080:8080 ghcr.io/ishaanaggrawal/beacon:latest`. The demo needs
  no network, and summaries fall back to the built-in ones.
- **Nothing works:** `docs/img/demo.png` is a screenshot of a full run. The
  CLI also tells the story: `beacon sample` prints six evidence cards and
  `beacon bench` prints the comparison table.

Handy links (both attacks on, auto-replay):

- `https://beacon-demo-8v3b.onrender.com/?hostile=1&sybil=1&play=320`
- `https://beacon-demo-8v3b.onrender.com/?hostile=1&play=320`
