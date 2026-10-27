#!/usr/bin/env python3
"""Brand gate: Redrob Film shows Redrob's name and marks, and nothing of upstream's.

1. No upstream trademark file (`docs/brand/`, removed as upstream's NOTICE requires).
2. The brand sources in assets/brand/ match assets/brand/SHA256SUMS byte for byte, and every app
   icon is one the brand script makes (no stray upstream icon left behind).
3. No upstream product name, company, app id or community link on a user-facing surface: string
   literals in the UI, the apps and the engine's messages, the translation catalogs, the packaging
   and the CI that builds it. Comments are not surfaces. What is kept on purpose (stored in files,
   or read back as an identifier) is listed in KEEP and in SOURCE_ORIGINS.md.
4. No upstream IDENTIFIER anywhere in the tree: crate names (`filmcraft-*`, `effectcraft-*`), the
   `.fcproj`/`.ecproj` extensions, `FILMCRAFT_*`/`EFFECTCRAFT_*` variables, `filmcraft://` URIs, the
   old MIME types and the `<EffectCraftProject>` root -- except on a line marked `// legacy:`, which
   is how the explicit read paths for old files and variables are allowed.

    python3 scripts/check_brand.py
"""
import hashlib
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
# The product names as people read them. Lowercase `filmcraft` is upstream's crate and identifier
# namespace (thread names, file-format ids, log targets) and stays; the installable names are
# checked through the packaging patterns below.
FORBIDDEN = re.compile(
    r"FilmCraft|Filmcraft|ArtCraft|EffectCraft|artcraft-mark|getartcraft|storytold/effectcraft|[Ss]toryteller|Learning Machines|(?i:discord)|(?:Exec|TryExec)=filmcraft|<binary>filmcraft|^command: filmcraft"
    # the product's former name (Redrob Cut, renamed to Redrob Film)
    r"|Redrob Cut|redrob-cut|redrob_cut|ai\.redrob\.cut|REDROB_CUT|RedrobCut"
)
SURFACES = (
    "crates/ui-egui/src/", "apps/filmcraft/src/", "apps/filmcraft-cli/src/",
    "crates/engine/src/", "crates/automation/src/", "packaging/", ".github/workflows/",
    "editors/effects/crates/",
)
# Kept on purpose: written into or read back from files, so changing them breaks existing projects
# and interchange round trips. SOURCE_ORIGINS.md lists them.
KEEP = re.compile(
    r"GPU Acceleration \(wgpu\)|Software Only \(CPU\)|CreatorTool|\"generator\""
    r"|filmcraft_|filmcraft-(target|install-|sign-|signing|logging-|web|speech|engine|ui-egui)"
    r"|/run/build/filmcraft/|FilmcraftApp|Filmcraft(Icon|Files|App|Exe|Cli|CliExe|Output)\b"
    # internal plist keys; the copyright credit upstream's licence requires; a negative test
    r"|FilmCraft(Version|BuildCommit)\b|the (FilmCraft|EffectCraft) contributors|desktop_file_exec_is_ours|effectcraft_|EffectCraftProject"
)
COMMENT = re.compile(r"^\s*(//|#|<!--|\*|/\*|;|REM\b)")

# 4. Upstream IDENTIFIERS (not names): crate names, file extensions, env-var prefixes, URI schemes,
# MIME types and the XML root. the de-upstream changes renamed them; the
# only place an old one may still appear is an explicit legacy READ path, whose line (or the line
# before or after it) carries a `// legacy:` (`# legacy:`, `<!-- legacy:`) marker, plus the
# attribution files and the scripts that know them.
IDENT_SKIP_FILES = (
    "NOTICE", "ATTRIBUTION.md", "LICENSE-APACHE", "LICENSE-MIT", "SOURCE_ORIGINS.md",
    "CHANGELOG.md", "editors/effects/NOTICE", "editors/effects/ATTRIBUTION.md", "editors/effects/LICENSE-APACHE",
    "editors/effects/LICENSE-MIT", "scripts/check_brand.py",
)
IDENT_SKIP_SUFFIXES = (".png", ".jpg", ".ico", ".icns", ".tiff", ".svg", ".ttf", ".otf", ".attribution", ".lock", ".fcproj", ".ecproj")
LEGACY = re.compile(r"(//|#|<!--)\s*legacy:")
# the versioned old-format fixtures the format crate's tests decode: inputs, not writers
LEGACY_FIXTURE = re.compile(r'fixture\("v\d+-[a-z-]+\.fcproj"\)')
# Identifiers stored in files and read back, kept on purpose (SOURCE_ORIGINS.md lists them): the JSON
# `format` ids and the OTIO metadata namespace.
IDENT_KEEP = re.compile(r"filmcraft\.(project|graphicsTemplate|export-presets)|\"filmcraft\"")


def old_crate_names():
    """The upstream names of every crate in the tree, derived from the Redrob names in place."""
    out = set()
    for pat in ("crates/*/Cargo.toml", "apps/*/Cargo.toml", "editors/effects/crates/*/Cargo.toml"):
        for m in ROOT.glob(pat):
            name = re.search(r'^name\s*=\s*"([^"]+)"', m.read_text(), re.M)
            if not name:
                continue
            name = name.group(1)
            for new, old in (("redrob-film-app", "filmcraft"), ("redrob-film-", "filmcraft-"), ("redrob-effects-", "effectcraft-")):
                if name == new or (new.endswith("-") and name.startswith(new)):
                    out.add(old + name[len(new):] if new.endswith("-") else old)
                    break
    return out


def upstream_ident_re():
    crates = sorted((n for n in old_crate_names() if n != "filmcraft"), key=len, reverse=True)
    dash = "|".join(re.escape(c) for c in crates)
    snake = "|".join(re.escape(c.replace("-", "_")) for c in crates)
    return re.compile(
        rf"(?<![A-Za-z0-9_-])(?:{dash})(?![A-Za-z0-9-])"      # crate names
        rf"|(?<![A-Za-z0-9_])(?:{snake})(?![A-Za-z0-9_])"     # snake_case crate paths (`use`, log targets)
        r"|(?:-p|--package)\s+filmcraft(?![A-Za-z0-9_.-])"   # the shell crate's old name in a -p list
        r'|^name\s*=\s*"filmcraft"'                         # ... or as a package name
        r"|\b(fcproj|ecproj|ecprojx)\b"                      # file extensions
        r"|\b(FILMCRAFT|EFFECTCRAFT)_[A-Z0-9_]+"              # env-var prefixes
        r"|(filmcraft|effectcraft)://"                       # URI schemes
        r"|application/x-(filmcraft|effectcraft)"            # MIME types
        r"|EffectCraftProject",                              # XML root
        re.M,
    )


def tracked_identifier_errors(tracked):
    errors = []
    ident = upstream_ident_re()
    for f in tracked:
        if f in IDENT_SKIP_FILES or f.endswith(IDENT_SKIP_SUFFIXES):
            continue
        try:
            text = (ROOT / f).read_text()
        except (UnicodeDecodeError, OSError):
            continue
        lines = text.split("\n")
        for n, line in enumerate(lines, 1):
            if not ident.search(line) or LEGACY_FIXTURE.search(line):
                continue
            # the marker may sit on the line itself or, where rustfmt moved a trailing comment, on
            # the line before or after
            if any(LEGACY.search(l) for l in lines[max(0, n - 2) : n + 1]):
                continue
            if not ident.search(IDENT_KEEP.sub("", line)):
                continue
            errors.append(f"{f}:{n}: upstream identifier left behind (rename it, or mark the legacy read path `// legacy:`): {line.strip()[:120]}")
    return errors


def files():
    out = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
    return [f for f in out if (ROOT / f).exists()]


def main() -> int:
    errors = []
    tracked = files()
    for prefix in ("docs/brand/",):
        for f in tracked:
            if f.startswith(prefix):
                errors.append(f"{f}: under the proprietary upstream prefix {prefix} (must be removed)")

    sums = (ROOT / "assets/brand/SHA256SUMS").read_text().split("\n")
    pinned = {}
    for line in filter(None, sums):
        digest, name = line.split(None, 1)
        pinned[name.strip()] = digest
    for name, digest in pinned.items():
        p = ROOT / "assets/brand" / name
        if not p.exists():
            errors.append(f"assets/brand/{name}: pinned but missing")
        elif hashlib.sha256(p.read_bytes()).hexdigest() != digest:
            errors.append(f"assets/brand/{name}: differs from SHA256SUMS (brand files are never edited)")
    for f in tracked:
        if f.startswith("assets/app-icon/") and not f.endswith((".attribution", "LICENSE.txt", "README.md")):
            name = pathlib.Path(f).name
            if not (name.startswith("redrob-film") or name.startswith("ai.redrob.film")):
                errors.append(f"{f}: not an icon packaging/brand-assets.sh makes")

    for f in tracked:
        if not f.startswith(SURFACES) or f.endswith((".png", ".tiff", ".ico", ".icns", ".DS_Store", ".attribution")):
            continue
        if f.startswith("packaging/macos/dmg/background"):
            continue  # outlined paths and the embedded lockup, not text
        try:
            text = (ROOT / f).read_text()
        except UnicodeDecodeError:
            continue
        tsv = f.endswith(".tsv")
        for n, line in enumerate(text.split("\n"), 1):
            if not FORBIDDEN.search(line) or KEEP.search(line) or COMMENT.match(line):
                continue
            if f.endswith(".rs"):
                # only string literals are surfaces in Rust; strip trailing comments first
                code = line.split("//", 1)[0] if '"' not in line.split("//", 1)[0][-1:] else line
                if not any(FORBIDDEN.search(s) for s in re.findall(r'"((?:[^"\\]|\\.)*)"', code)):
                    continue
            if tsv and line.startswith("#"):
                continue
            errors.append(f"{f}:{n}: upstream brand on a user-facing surface: {line.strip()[:120]}")

    errors.extend(tracked_identifier_errors(tracked))

    if errors:
        print("\n".join(errors))
        print(f"brand: {len(errors)} problem(s)", file=sys.stderr)
        return 1
    print(f"brand: ok ({len(pinned)} pinned brand sources)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
