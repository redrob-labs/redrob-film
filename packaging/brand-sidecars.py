#!/usr/bin/env python3
"""Write the `<file>.attribution` sidecars for the Redrob brand files (`cargo xtask assets`).

Run by packaging/brand-assets.sh after it derives the icons. The sources in assets/brand/ are
verbatim shipped Redrob files; everything under assets/app-icon/ is derived
from them by that script. Rows for ATTRIBUTION.md are printed with --rows.
"""
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ADDED = "2026-10-12 for Janghoon Lee (Redrob)"
LICENSE = "Redrob trademark; not covered by the repository's MIT OR Apache-2.0 licence (assets/brand/README.md)"
AUTHOR = "Redrob (McKinley Rice, Inc.)"

SOURCES = {
    "redrob-film-icon-web.svg": "Redrob Film app icon, rounded tile (icon-web), vector",
    "redrob-film-icon-web-1024.png": "Redrob Film app icon, rounded tile (icon-web), 1024 px Chromium render",
    "redrob-logo-primary.png": "Redrob primary lockup, for light grounds",
    "redrob-logo-primary-white.png": "Redrob primary lockup, white wordmark for dark grounds",
}
UI = "@redrob-labs/ui 1.8.0, dist/brand/app-icons/redrob-film/redrob-film-icon-web.svg"
DS_SOURCE = {
    "redrob-film-icon-web.svg": (UI + ", copied verbatim", "2026-10-20 for Janghoon Lee (Redrob)"),
    "redrob-film-icon-web-1024.png": (UI + ", rendered at 1024 px by Chromium (keeps the gradients)", "2026-10-20 for Janghoon Lee (Redrob)"),
    "redrob-logo-primary.png": ("Redrob design system, assets/Logos/redrob-logo-primary.png, copied verbatim", ADDED),
    "redrob-logo-primary-white.png": ("Redrob design system, assets/Logos/redrob-logo-primary-white.png, copied verbatim", ADDED),
}
DERIVED_ADDED = "2026-10-20 for Janghoon Lee (Redrob)"


def derived():
    out = []
    a = ROOT / "assets/app-icon"
    for p in sorted(a.rglob("*")):
        if p.is_file() and p.name not in ("LICENSE.txt", "README.md") and not p.name.endswith(".attribution"):
            out.append((p, f"Redrob Film app icon ({p.relative_to(a)}), derived from the Redrob Film icon-web tile"))
    return out


def sidecar(path: pathlib.Path, title: str, source: str, notes: str, added: str) -> None:
    text = (
        f"asset:        {path.name}\n"
        f"title:        {title}\n"
        f"author:       {AUTHOR}\n"
        f"source:       {source}\n"
        f"license:      {LICENSE}\n"
        f"added:        {added}\n"
        f"notes:        {notes}\n"
    )
    pathlib.Path(str(path) + ".attribution").write_text(text)


def main() -> None:
    rows = []
    for name, title in SOURCES.items():
        p = ROOT / "assets/brand" / name
        source, added = DS_SOURCE[name]
        sidecar(p, title, source, "Pinned by assets/brand/SHA256SUMS. Never redrawn or recolored.", added)
        rows.append((p, title))
    for p, title in derived():
        sidecar(p, title, "derived by packaging/brand-assets.sh from assets/brand/ (copied as is, or resized down)", "Regenerate with packaging/brand-assets.sh; do not edit by hand.", DERIVED_ADDED)
        rows.append((p, title))
    if "--rows" in sys.argv:
        for p, title in rows:
            print(f"| `{p.relative_to(ROOT)}` | {AUTHOR} | {title} | Redrob trademark (`assets/brand/README.md`) |")


if __name__ == "__main__":
    main()
