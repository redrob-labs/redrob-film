#!/usr/bin/env python3
"""Real-world media corpus (docs/corpus.md, gaps.md G6).

    scripts/corpus.py check                       validate tests/corpus/manifest.toml
    scripts/corpus.py list [--tier T]             one line per entry
    scripts/corpus.py generate --out DIR [--tier small|large|all] [--only ID ...] [--force]
    scripts/corpus.py verify --out DIR            sha256 of every present file; pinned ones checked

`generate` runs ffmpeg as a black box (never linked or shipped, AGENTS.md section 2) and writes
DIR/SHA256SUMS for the files it made. Small-tier files must stay under 2 MB. A generator that
fails (an encoder missing from this ffmpeg build) is reported and skipped; the acceptance
harness then skips that file. Exit status 1 when the manifest is invalid, a small file is too
large, or a pinned sha256 does not match.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import random
import shlex
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "tests" / "corpus" / "manifest.toml"
SMALL_LIMIT = 2 * 1024 * 1024
MIN_ENTRIES = 50

REQUIRED = ("id", "kind", "title", "codec", "container", "tier", "file", "video", "audio", "expect")
KINDS = {"phone", "camera", "cinema", "screen", "audio", "container", "multitrack", "still", "damaged"}
TIERS = {"small", "large", "real"}
EXPECTS = {"pass", "error", "report"}
KNOWN = set(REQUIRED) | {"issue", "generate", "post", "obtain", "sha256", "size", "fps", "rate", "seconds"}


def load() -> list[dict]:
    with MANIFEST.open("rb") as f:
        return tomllib.load(f).get("media", [])


def check(entries: list[dict]) -> list[str]:
    errs = []
    seen_ids: set[str] = set()
    seen_files: set[str] = set()
    for i, e in enumerate(entries):
        where = e.get("id", f"entry {i}")
        for k in REQUIRED:
            if k not in e:
                errs.append(f"{where}: missing `{k}`")
        for k in e:
            if k not in KNOWN:
                errs.append(f"{where}: unknown key `{k}`")
        if e.get("id") in seen_ids:
            errs.append(f"{where}: duplicate id")
        seen_ids.add(e.get("id"))
        f = e.get("file", "")
        if f in seen_files:
            errs.append(f"{where}: duplicate file {f}")
        seen_files.add(f)
        if f.startswith("/") or ".." in Path(f).parts:
            errs.append(f"{where}: `file` must be relative to the corpus directory")
        if e.get("kind") not in KINDS:
            errs.append(f"{where}: kind `{e.get('kind')}` not in {sorted(KINDS)}")
        if e.get("tier") not in TIERS:
            errs.append(f"{where}: tier `{e.get('tier')}` not in {sorted(TIERS)}")
        if e.get("expect") not in EXPECTS:
            errs.append(f"{where}: expect `{e.get('expect')}` not in {sorted(EXPECTS)}")
        if e.get("tier") == "small" and "generate" not in e:
            errs.append(f"{where}: a small entry needs `generate`")
        if e.get("tier") in ("large", "real") and "generate" not in e and "obtain" not in e:
            errs.append(f"{where}: say how to get it (`generate` or `obtain`)")
        if e.get("tier") == "real" and "obtain" not in e:
            errs.append(f"{where}: a real entry needs `obtain`")
        for k in ("video", "audio"):
            if k in e and not isinstance(e[k], bool):
                errs.append(f"{where}: `{k}` must be true or false")
        for k in ("fps", "rate", "seconds"):
            if k in e and (not isinstance(e[k], int) or isinstance(e[k], bool) or e[k] <= 0):
                errs.append(f"{where}: `{k}` must be a positive integer")
    if len(entries) < MIN_ENTRIES:
        errs.append(f"{len(entries)} entries, want at least {MIN_ENTRIES}")
    return errs


def ffmpeg_args(e: dict, out: Path) -> list[str]:
    size = e.get("size", "640x360")
    fps = e.get("fps", 30)
    rate = e.get("rate", 48000)
    secs = e.get("seconds", 2)
    v = f"-f lavfi -i testsrc2=s={size}:r={fps}:d={secs}"
    a = f"-f lavfi -i sine=f=1000:r={rate}:d={secs}"
    text = e["generate"].replace("{V}", v).replace("{A}", a).replace("{S}", str(secs))
    text = text.replace("{size}", str(size)).replace("{fps}", str(fps)).replace("{rate}", str(rate))
    return ["-hide_banner", "-loglevel", "error", "-y", *shlex.split(text), str(out)]


def post(op: str, ffmpeg: str, path: Path, seed_name: str) -> None:
    name, _, arg = op.partition(":")
    if name == "rotate":
        tmp = path.with_name(path.stem + ".rot" + path.suffix)
        cmd = [ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-display_rotation", arg, "-i", str(path), "-c", "copy", str(tmp)]
        subprocess.run(cmd, check=True)
        tmp.replace(path)
    elif name == "truncate":
        data = path.read_bytes()
        path.write_bytes(data[: int(len(data) * float(arg))])
    elif name == "flip":
        seed, _, count = arg.partition(":")
        data = bytearray(path.read_bytes())
        rng = random.Random(int(seed))
        start = min(4096, len(data) // 4)  # past the container header, so it still opens
        for _ in range(int(count)):
            if len(data) > start:
                i = rng.randrange(start, len(data))
                data[i] ^= 1 << rng.randrange(8)
        path.write_bytes(bytes(data))
    elif name == "zero":
        path.write_bytes(b"")
    elif name == "random":
        rng = random.Random(seed_name)
        path.write_bytes(bytes(rng.randrange(256) for _ in range(int(arg))))
    else:
        raise ValueError(f"unknown post step `{op}`")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def generate(entries: list[dict], out: Path, tier: str, only: list[str], force: bool) -> int:
    ffmpeg = os.environ.get("REDROB_FILM_FFMPEG") or shutil.which("ffmpeg")
    if not ffmpeg:
        print("ffmpeg not found (set REDROB_FILM_FFMPEG)", file=sys.stderr)
        return 1
    status = 0
    made, failed, kept = [], [], []
    sums = []
    for e in entries:
        if "generate" not in e or (only and e["id"] not in only):
            continue
        if tier != "all" and e["tier"] != tier:
            continue
        path = out / e["file"]
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists() and not force:
            kept.append(e["id"])
        else:
            tmp = path.with_name(path.stem + ".tmp" + path.suffix)
            r = subprocess.run([ffmpeg, *ffmpeg_args(e, tmp)], capture_output=True, text=True)
            if r.returncode != 0 or not tmp.exists():
                failed.append(e["id"])
                print(f"SKIP {e['id']}: ffmpeg failed: {r.stderr.strip()[-300:]}", file=sys.stderr)
                tmp.unlink(missing_ok=True)
                continue
            if "post" in e:
                try:
                    post(e["post"], ffmpeg, tmp, e["id"])
                except (subprocess.CalledProcessError, OSError, ValueError) as err:
                    failed.append(e["id"])
                    print(f"SKIP {e['id']}: post step `{e['post']}` failed: {err}", file=sys.stderr)
                    tmp.unlink(missing_ok=True)
                    continue
            tmp.replace(path)
            made.append(e["id"])
        size = path.stat().st_size
        if e["tier"] == "small" and size > SMALL_LIMIT:
            print(f"TOO BIG {e['id']}: {size} bytes > {SMALL_LIMIT}", file=sys.stderr)
            status = 1
        sums.append(f"{sha256(path)}  {e['file']}")
    (out / "SHA256SUMS").write_text("\n".join(sums) + "\n")
    print(f"made {len(made)}, kept {len(kept)}, failed {len(failed)} in {out}")
    if failed:
        print("failed: " + " ".join(failed))
    return status


def verify(entries: list[dict], out: Path) -> int:
    status = 0
    present = 0
    for e in entries:
        path = out / e["file"]
        if not path.exists():
            continue
        present += 1
        got = sha256(path)
        want = e.get("sha256")
        if want and want != got:
            print(f"MISMATCH {e['id']}: {got} != pinned {want}", file=sys.stderr)
            status = 1
    print(f"{present} of {len(entries)} corpus files present in {out}")
    return status


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check")
    ls = sub.add_parser("list")
    ls.add_argument("--tier", choices=sorted(TIERS))
    g = sub.add_parser("generate")
    g.add_argument("--out", type=Path, required=True)
    g.add_argument("--tier", choices=["small", "large", "all"], default="small")
    g.add_argument("--only", nargs="*", default=[])
    g.add_argument("--force", action="store_true")
    v = sub.add_parser("verify")
    v.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    entries = load()
    errs = check(entries)
    if errs:
        for e in errs:
            print(f"manifest: {e}", file=sys.stderr)
        return 1
    if args.cmd == "check":
        tiers = {t: sum(1 for e in entries if e["tier"] == t) for t in sorted(TIERS)}
        print(f"manifest ok: {len(entries)} entries {tiers}")
        return 0
    if args.cmd == "list":
        for e in entries:
            if args.tier is None or e["tier"] == args.tier:
                print(f"{e['id']:<36} {e['tier']:<6} {e['expect']:<7} {e['file']}")
        return 0
    if args.cmd == "generate":
        return generate(entries, args.out.resolve(), args.tier, args.only, args.force)
    return verify(entries, args.out.resolve())


if __name__ == "__main__":
    sys.exit(main())
