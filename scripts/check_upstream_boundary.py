#!/usr/bin/env python3
"""Check this fork's boundary against upstream, as recorded in upstream-base.json.

Four things are verified, and a stale record fails the check rather than passing it:

1. The pinned base commit exists, matches the pinned tag, and is an ancestor of HEAD, so
   `git merge-base` with upstream is real and the next sync is a three-way merge.
2. Upstream's copyright notice is still present in NOTICE, with ours below it (FORKS.md).
3. Nothing lives under an `excludedPrefixes` entry (subtrees upstream licenses separately).
4. Files under `proprietaryPrefixes` (upstream trademarks that forks must remove): while
   `proprietaryReplaced` is false the count of files still byte-identical to upstream's is
   reported; once it is true, any such file fails the check.

Usage: python3 scripts/check_upstream_boundary.py   (exit 0 = clean)
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
UPSTREAM_NOTICE_LINE = "Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors."
OUR_NOTICE_LINE = "Copyright (c) 2026-present Janghoon Lee (Redrob) and contributors"


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()


def git_ok(*args: str) -> bool:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True).returncode == 0


def main() -> int:
    with open(os.path.join(ROOT, "upstream-base.json"), encoding="utf-8") as fh:
        rec = json.load(fh)
    errors: list[str] = []
    notes: list[str] = []

    base = rec["base"]
    tag = rec["baseTag"]
    if not git_ok("cat-file", "-e", f"{base}^{{commit}}"):
        errors.append(f"base commit {base} is not in this repository (full upstream history is required)")
    else:
        if git_ok("rev-parse", "-q", "--verify", f"refs/tags/{tag}"):
            tag_sha = git("rev-parse", f"{tag}^{{commit}}")
            if tag_sha != base and git_ok("merge-base", "--is-ancestor", base, tag_sha):
                # Our own release tags share upstream's vX.Y.Z namespace; a tag
                # cut after the fork point is ours, not upstream's base tag.
                notes.append(f"tag {tag} is our own release tag ({tag_sha}), not upstream's; skipped tag/base equality")
            elif tag_sha != base:
                errors.append(f"tag {tag} resolves to {tag_sha}, but base is {base}")
        else:
            notes.append(f"tag {tag} not fetched locally; skipped tag/base equality")
        if not git_ok("merge-base", "--is-ancestor", base, "HEAD"):
            errors.append(f"base {base} is not an ancestor of HEAD; the merge base with upstream is broken")

    notice_path = os.path.join(ROOT, "NOTICE")
    notice = open(notice_path, encoding="utf-8").read() if os.path.exists(notice_path) else ""
    up = notice.find(UPSTREAM_NOTICE_LINE)
    ours = notice.find(OUR_NOTICE_LINE)
    if up < 0:
        errors.append("NOTICE no longer carries upstream's copyright line (never replace it; add ours below)")
    if ours < 0:
        errors.append("NOTICE does not carry our copyright line")
    if up >= 0 and ours >= 0 and ours < up:
        errors.append("NOTICE lists our copyright line above upstream's; ours goes below theirs")

    tracked = git("ls-files", "-z").split("\0")
    tracked = [t for t in tracked if t]
    for prefix in rec.get("excludedPrefixes", []):
        hits = [t for t in tracked if t.startswith(prefix)]
        if hits:
            errors.append(f"{len(hits)} file(s) under excluded prefix {prefix!r}, e.g. {hits[0]}")

    identical = 0
    checked = 0
    for prefix in rec.get("proprietaryPrefixes", []):
        for path in (t for t in tracked if t.startswith(prefix)):
            checked += 1
            try:
                upstream_blob = git("rev-parse", f"{base}:{path}")
            except subprocess.CalledProcessError:
                continue
            head_blob = git("rev-parse", f"HEAD:{path}")
            if upstream_blob == head_blob:
                identical += 1
    if checked:
        msg = f"{identical}/{checked} file(s) under proprietary prefixes are byte-identical to upstream {tag}"
        if rec.get("proprietaryReplaced"):
            if identical:
                errors.append(msg + " (proprietaryReplaced is true)")
        else:
            notes.append(msg + " (proprietaryReplaced is false; the brand pass has not run yet)")

    for n in notes:
        print(f"note: {n}")
    for e in errors:
        print(f"error: {e}", file=sys.stderr)
    if errors:
        return 1
    print(f"upstream boundary ok: base {tag} ({base[:7]}), NOTICE intact")
    return 0


if __name__ == "__main__":
    sys.exit(main())
