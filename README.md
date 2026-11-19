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

> **Status:** early. Right now beacon can read and query Vouch files. The trust
> graph, campaign detection and web demo are in progress.

## Try it

```sh
cargo run -- inspect data/ghostty/VOUCHED.td
cargo run -- check data/ghostty/VOUCHED.td mitchellh   # exit 0 vouched, 1 denounced, 2 unknown
```

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Changes land through pull requests; `main` is never pushed to directly.

## License

MIT
