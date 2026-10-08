#!/usr/bin/env python3
"""Convert swift/Localizations/<lang>.lproj/Localizable.strings into windows/ui/i18n/<lang>.json.

The Swift files stay the source of truth (English text is the key). Run after editing them:
    python3 scripts/gen-win-i18n.py
"""
import json
import pathlib
import re

root = pathlib.Path(__file__).resolve().parent.parent
out = root / "windows" / "ui" / "i18n"
out.mkdir(parents=True, exist_ok=True)
pair = re.compile(r'^"((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)";\s*$')


def unescape(s: str) -> str:
    return s.replace('\\"', '"').replace("\\n", "\n").replace("\\\\", "\\")


for lproj in sorted((root / "swift" / "Localizations").glob("*.lproj")):
    lang = lproj.name.removesuffix(".lproj")
    if lang == "en":
        continue
    d = {}
    for line in (lproj / "Localizable.strings").read_text(encoding="utf-8").splitlines():
        m = pair.match(line.strip())
        if m:
            d[unescape(m.group(1))] = unescape(m.group(2))
    (out / f"{lang}.json").write_text(json.dumps(d, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{lang}: {len(d)} strings")
