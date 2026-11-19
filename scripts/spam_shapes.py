"""Collect aggregate shape statistics of spam-labelled PRs via the GitHub API.

Read-only, through an authenticated `gh` CLI. Keeps only aggregates: no PR
text, usernames or repository names end up in the output. Results are in
data/research/spam_shapes.json and discussed in docs/BENCHMARKS.md.

Usage: python scripts/spam_shapes.py data/research/spam_shapes.json
"""
import json, os, subprocess, sys, collections, datetime as dt

GH = os.environ.get("GH", "gh")

def api(path, **params):
    args = [GH, "api", "-X", "GET", path]
    for k, v in params.items():
        args += ["-f", f"{k}={v}"]
    out = subprocess.run(args, capture_output=True, text=True, encoding="utf-8")
    if out.returncode != 0:
        return None
    return json.loads(out.stdout)

def parse_t(s):
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))

def search(q, pages):
    items = []
    for p in range(1, pages + 1):
        r = api("search/issues", q=q, per_page=100, page=p)
        if not r or not r.get("items"):
            break
        items += r["items"]
    return items

DOC_EXT = {".md", ".txt", ".rst", ".adoc", ""}

def ext(name):
    base = name.rsplit("/", 1)[-1]
    return "." + base.rsplit(".", 1)[-1].lower() if "." in base else ""

def pct(xs, ps=(10, 25, 50, 75, 90)):
    xs = sorted(xs)
    if not xs:
        return {}
    return {f"p{p}": xs[min(len(xs) - 1, int(len(xs) * p / 100))] for p in ps}

def analyze(label, q, search_pages, detail_n):
    items = search(q, search_pages)
    # Coordination: per-author bursts across repos (computed in memory only).
    by_author = collections.defaultdict(list)
    for it in items:
        repo = it["repository_url"].rsplit("/", 2)
        by_author[it["user"]["login"]].append((parse_t(it["created_at"]), "/".join(repo[-2:])))
    multi = [v for v in by_author.values() if len(v) > 1]
    repos_per_multi = [len({r for _, r in v}) for v in multi]
    gaps_h = []
    for v in multi:
        v.sort()
        gaps_h += [(b[0] - a[0]).total_seconds() / 3600 for a, b in zip(v, v[1:])]

    files_n, churn, hunks, docs_only, readme_only, ext_counts, age_days = [], [], [], 0, 0, collections.Counter(), []
    seen_users = {}
    sample = items[:detail_n]
    for it in sample:
        owner_repo = "/".join(it["repository_url"].rsplit("/", 2)[-2:])
        num = it["number"]
        files = api(f"repos/{owner_repo}/pulls/{num}/files", per_page=100)
        if files is None:
            continue
        files_n.append(len(files))
        churn.append(sum(f.get("additions", 0) + f.get("deletions", 0) for f in files))
        hunks.append(sum((f.get("patch") or "").count("\n@@") + (1 if (f.get("patch") or "").startswith("@@") else 0) for f in files))
        exts = [ext(f["filename"]) for f in files]
        ext_counts.update(set(exts))
        if files and all(e in DOC_EXT for e in exts):
            docs_only += 1
        if files and all(f["filename"].rsplit("/", 1)[-1].lower().startswith("readme") for f in files):
            readme_only += 1
        login = it["user"]["login"]
        if login not in seen_users:
            u = api(f"users/{login}")
            seen_users[login] = parse_t(u["created_at"]) if u and u.get("created_at") else None
        if seen_users[login]:
            age_days.append((parse_t(it["created_at"]) - seen_users[login]).days)

    n = len(files_n)
    return {
        "label": label,
        "query": q,
        "prs_seen": len(items),
        "authors_seen": len(by_author),
        "authors_with_multiple_prs": len(multi),
        "share_of_prs_from_repeat_authors": round(sum(len(v) for v in multi) / max(1, len(items)), 3),
        "distinct_repos_per_repeat_author": pct(repos_per_multi),
        "hours_between_prs_of_same_author": pct([round(g, 2) for g in gaps_h]),
        "detailed_sample": n,
        "files_changed": pct(files_n),
        "lines_changed": pct(churn),
        "hunks": pct(hunks),
        "share_docs_only": round(docs_only / max(1, n), 3),
        "share_readme_only": round(readme_only / max(1, n), 3),
        "top_extensions": [[e or "(none)", c] for e, c in ext_counts.most_common(8)],
        "account_age_days_at_pr": pct(age_days),
        "share_accounts_under_30_days": round(sum(a < 30 for a in age_days) / max(1, len(age_days)), 3),
    }

if __name__ == "__main__":
    out = {
        "collected": dt.date.today().isoformat(),
        "note": "Aggregates only. No PR text, usernames or repository names are stored.",
        "samples": [
            analyze("hacktoberfest-2020-spam", "is:pr label:spam created:2020-10-01..2020-10-31", 10, 120),
            analyze("ai-era-slop", "is:pr label:slop created:2025-01-01..2026-11-19", 10, 120),
            analyze("ai-era-ai-slop", 'is:pr label:"ai-slop"', 7, 120),
        ],
    }
    with open(sys.argv[1], "w", encoding="utf-8", newline="\n") as f:
        json.dump(out, f, indent=2)
    print(json.dumps(out, indent=2))
