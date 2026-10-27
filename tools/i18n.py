#!/usr/bin/env python3
"""Übersetzbare Texte von tvjoe sammeln und die Kataloge abgleichen.

Sucht in crates/*/src/**/*.rs nach tr("…"), tf!("…", …) und trn("…", "…", …), schreibt die
Vorlage po/tvjoe.pot und gleicht jede po/<sprache>.po mit msgmerge ab (neue Texte kommen leer
hinzu, entfallene werden auskommentiert).

    tools/i18n.py            Vorlage schreiben und Kataloge abgleichen
    tools/i18n.py --check    nur prüfen: Vorlage aktuell? fehlende Übersetzungen je Sprache
"""
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PO = ROOT / "po"
POT = PO / "tvjoe.pot"

STR = r'"((?:[^"\\]|\\.)*)"'
PATTERNS = [
    ("tr", re.compile(r"\btr\(\s*" + STR + r"\s*,?\s*\)", re.S)),
    ("tf", re.compile(r"\btf!\(\s*" + STR, re.S)),
    ("trn", re.compile(r"\btrn\(\s*" + STR + r"\s*,\s*" + STR, re.S)),
]


def rust_unescape(s):
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        n = s[i + 1]
        if n == "\n":  # Zeilenfortsetzung: folgende Leerzeichen entfallen
            i += 2
            while i < len(s) and s[i] in " \t\n":
                i += 1
            continue
        if n == "u":
            end = s.index("}", i)
            out.append(chr(int(s[i + 3:end], 16)))
            i = end + 1
            continue
        out.append({"n": "\n", "t": "\t", "r": "\r", "0": "\0"}.get(n, n))
        i += 2
    return "".join(out)


def po_quote(s):
    s = s.replace("\\", "\\\\").replace('"', '\\"').replace("\t", "\\t")
    if "\n" not in s:
        return '"' + s + '"'
    parts = s.split("\n")
    lines = [p + "\\n" for p in parts[:-1]] + ([parts[-1]] if parts[-1] else [])
    return '""\n' + "\n".join('"' + l + '"' for l in lines)


def collect():
    entries = {}  # msgid -> (plural or None, [refs])
    files = sorted(ROOT.glob("crates/*/src/**/*.rs"))
    for f in files:
        text = f.read_text(encoding="utf-8")
        rel = f.relative_to(ROOT).as_posix()
        if rel.endswith("i18n.rs"):
            continue
        for kind, pat in PATTERNS:
            for m in pat.finditer(text):
                line = text.count("\n", 0, m.start()) + 1
                msgid = rust_unescape(m.group(1))
                plural = rust_unescape(m.group(2)) if kind == "trn" else None
                e = entries.setdefault(msgid, [None, []])
                if plural is not None:
                    e[0] = plural
                e[1].append(f"{rel}:{line}")
    return entries


def pot_text(entries):
    out = [
        "# Vorlage der übersetzbaren Texte von tvjoe (erzeugt von tools/i18n.py).",
        'msgid ""',
        'msgstr ""',
        '"Project-Id-Version: tvjoe\\n"',
        '"MIME-Version: 1.0\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
        "",
    ]
    for msgid in sorted(entries, key=lambda k: (entries[k][1][0], k)):
        plural, refs = entries[msgid]
        out.append("#: " + " ".join(refs))
        if "{" in msgid or (plural and "{" in plural):
            out.append("#, no-c-format")
        out.append("msgid " + po_quote(msgid))
        if plural is not None:
            out.append("msgid_plural " + po_quote(plural))
            out.append('msgstr[0] ""')
            out.append('msgstr[1] ""')
        else:
            out.append('msgstr ""')
        out.append("")
    return "\n".join(out)


def main():
    check = "--check" in sys.argv
    text = pot_text(collect())
    ok = True
    if check:
        if not POT.exists() or POT.read_text(encoding="utf-8") != text:
            print("✖ po/tvjoe.pot ist nicht aktuell (tools/i18n.py ausführen)")
            ok = False
    else:
        POT.write_text(text, encoding="utf-8")
        print(f"» {POT.relative_to(ROOT)}: {text.count(chr(10) + 'msgid ') - 1} Texte")
    for po in sorted(PO.glob("*.po")):
        if not check:
            subprocess.run(["msgmerge", "--quiet", "--update", "--backup=none",
                            "--no-fuzzy-matching", str(po), str(POT)], check=True)
        r = subprocess.run(["msgfmt", "--check-format", "--statistics", "-o", "/dev/null",
                            str(po)], capture_output=True, text=True)
        print(f"  {po.name}: {r.stderr.strip()}")
        if r.returncode != 0 or "untranslated" in r.stderr or "fuzzy" in r.stderr:
            ok = False
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
