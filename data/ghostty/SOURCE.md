# Ghostty vouch list fixture

Read-only snapshot of Ghostty's public trust list, used as a real-world anchor for
tests and benchmarks. Nothing in beacon writes back to Ghostty.

| File | Source |
|---|---|
| `VOUCHED.td` | `https://github.com/ghostty-org/ghostty/blob/main/.github/VOUCHED.td` at commit `3f2fe1472882349ca140a83deca150573a7018fb` |
| `history.jsonl` | Every commit touching that file, from the GitHub REST API (`/repos/ghostty-org/ghostty/commits?path=.github/VOUCHED.td`), oldest first. Only `sha`, `date` and `message` are kept. |

Fetched 2026-11-19. Ghostty is MIT-licensed. Ghostty has announced it is moving off
GitHub; the GitHub repo stays up as a read-only mirror, so these URLs should keep working.

## What's in it

- 425 entries: 397 vouched, 28 denounced. 19 of the denouncements give a reason;
  one vouched entry also carries a note.
- No entry uses a `platform:` prefix, so everything is implicitly GitHub.
- Handles are stored lowercase; commit messages keep the original casing.
- 382 commits from 2026-02-15 to 2026-11-18. Most are written by a bot and name
  the maintainer who triggered them and the action taken:

  ```
  Triggered by [comment](https://github.com/ghostty-org/ghostty/issues/...)
  from @mitchellh.

  Vouch: @MrMage
  ```

  322 commits parse this way (290 vouches, 28 denouncements, 4 unvouches). The rest
  are the initial import, early bot commits that only link the source thread, weekly
  "Sync CODEOWNERS" commits that vouch every code owner in bulk, and a couple of
  hand edits.

## Refreshing

```sh
curl -sfL https://raw.githubusercontent.com/ghostty-org/ghostty/main/.github/VOUCHED.td -o VOUCHED.td
```

Then re-pull the commit list (4 pages of 100) and update the counts in
`tests/ghostty_fixture.rs`.
