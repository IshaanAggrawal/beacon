# beacon

A trust network for open-source projects that keeps coordinated PR and issue spam out
without locking newcomers out.

Each project keeps its own signed list of who it vouches for and who it has
denounced, in its own Git repo. Projects choose which other projects they trust and
how much. When a pull request comes in, beacon combines what the projects you trust
know with cross-project behavior, then gives one of three answers:

- **Trusted:** someone you trust, directly or through projects you trust, vouches for them.
- **Clean newcomer:** nobody knows them yet, and nothing looks wrong. Never auto-blocked.
- **Quarantined:** several independent sources denounce them, or their PR is part of
  a campaign spanning many repos.

Each answer explains itself in plain English. It also warns you when a verdict rests
on a single source.

beacon builds on [Vouch](https://github.com/mitchellh/vouch) and reads its
`VOUCHED.td` format directly. See [docs/RESEARCH.md](docs/RESEARCH.md) for what we
learned from Ghostty's real list and what a network adds over a flat list.

> **Status:** early. Signed ledgers, Vouch import, peer sync, the trust graph and
> campaign detection work. Explanations, the LLM explainer and the web demo are
> in progress.

## Try it

Read an existing Vouch file:

```sh
cargo run -- inspect data/ghostty/VOUCHED.td
cargo run -- check data/ghostty/VOUCHED.td mitchellh   # exit 0 vouched, 1 denounced, 2 unknown
```

Set up a project and connect it to another one:

```sh
beacon init myproject --maintainer alice        # writes .trust/ and a signing key
beacon import .github/VOUCHED.td                # bring over an existing Vouch list
beacon vouch github:newcomer --reason "solid first PR"
beacon vouch github:student --provisional 30 --scope docs
beacon denounce github:spammer --reason "same PR opened in 30 repos"
beacon trust zig 0.8 --url https://github.com/ziglang/zig
beacon sync                                     # fetch peers' signed ledgers (read-only)
beacon peers                                    # how much you trust each project, and why
beacon why github:newcomer                      # what the projects you trust say about someone
beacon verify                                   # check every signature and the hash chain
```

Compare beacon with flat Vouch lists on a simulated federation:

```sh
cargo run --release -- bench --seed 1 --seeds 5
```

On five seeds, beacon caught 94–95% of coordinated campaign spam, blocked no
honest PRs, and ignored every smear from a hostile project and a 40-project
Sybil ring. Requiring a vouch blocked every newcomer. Results, real-world spam
statistics and the limits of all this are in
[docs/BENCHMARKS.md](docs/BENCHMARKS.md).

How it works and why it's built this way: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Changes land through pull requests; `main` is never pushed to directly.

## License

MIT
