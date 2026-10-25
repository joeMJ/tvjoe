#!/usr/bin/env python3
"""Aktualisiert die Tabelle der Rust-Abhängigkeiten in THIRD_PARTY.md (alle Plattformen und Merkmale,
Lizenzangabe laut Paketbeschreibung; der übrige Text der Datei bleibt unverändert) und schreibt
THIRD_PARTY_LICENSES.txt mit den Lizenztexten aus den Paketquellen (LICENSE*, COPYING*, NOTICE*, UNLICENSE)
sowie den Texten der eingebauten C-Bibliotheken (xz, SQLite).
Gleiche Texte stehen nur einmal, darüber die Pakete, für die sie gelten.
Vorlage: tools/third-party.py aus remotejoe.
Aufruf im Hauptordner des Repositorys:  python3 -I tools/third-party.py"""
import hashlib
import json
import os
import subprocess

DATEI = "THIRD_PARTY.md"
TEXTE = "THIRD_PARTY_LICENSES.txt"
meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"], text=True))
eigene = {p["id"] for p in meta["packages"] if p["source"] is None}
fremde = sorted((p for p in meta["packages"] if p["id"] not in eigene), key=lambda p: (p["name"], p["version"]))
zeilen = sorted({f"| {p['name']} | {p['version']} | {p.get('license') or 'unbekannt'} |" for p in fremde})

text = open(DATEI, encoding="utf-8").read().split("\n")
kopf = text.index("| Paket | Version | Lizenz |")
ende = kopf + 2
while ende < len(text) and text[ende].startswith("|"):
    ende += 1
text[kopf + 2:ende] = zeilen
open(DATEI, "w", encoding="utf-8").write("\n".join(text))
print(f"{len(zeilen)} Pakete eingetragen")

# Lizenztexte: je Paket die Lizenzdateien im Quellordner; gleiche Texte zusammengefasst.
PRAEFIXE = ("license", "licence", "copying", "notice", "unlicense", "copyright")
texte = {}  # Prüfsumme -> (Dateiname, Text, [Pakete])
ohne = []


def aufnehmen(pfad, name):
    inhalt = open(pfad, encoding="utf-8", errors="replace").read().replace("\r\n", "\n").strip()
    h = hashlib.sha256(inhalt.encode()).hexdigest()
    texte.setdefault(h, (os.path.basename(pfad), inhalt, []))[2].append(name)


for p in fremde:
    ordner = os.path.dirname(p["manifest_path"])
    dateien = sorted(f for f in os.listdir(ordner) if f.lower().startswith(PRAEFIXE) and os.path.isfile(os.path.join(ordner, f)))
    name = f"{p['name']} {p['version']}"
    if not dateien:
        ohne.append(f"{name} ({p.get('license') or 'unbekannt'})")
    for f in dateien:
        aufnehmen(os.path.join(ordner, f), name)
    # Eingebaute C-Bibliothek xz (liblzma): Lizenztext aus dem mitgelieferten Quellbaum.
    if p["name"] == "liblzma-sys":
        xz = os.path.join(ordner, "xz", "COPYING.0BSD")
        if os.path.isfile(xz):
            aufnehmen(xz, f"xz/liblzma (in {name})")

linie = "=" * 78
aus = ["Lizenztexte der Bibliotheken Dritter in tvjoe",
       "Erzeugt mit tools/third-party.py aus den Quellen der Rust-Pakete (alle Plattformen und Merkmale).",
       "Übersicht mit Lizenzangaben: THIRD_PARTY.md.", ""]
for f, inhalt, pakete in sorted(texte.values(), key=lambda t: (t[2][0], t[0])):
    aus += [linie, f"{f} – gilt für: {', '.join(pakete)}", linie, inhalt, ""]
aus += [linie, "SQLite (eingebaut über libsqlite3-sys)", linie,
        "SQLite ist gemeinfrei (Public Domain), siehe https://sqlite.org/copyright.html", ""]
if ohne:
    aus += [linie, "Pakete ohne eigene Lizenzdatei (es gilt der Standardtext der genannten Lizenz, siehe https://spdx.org/licenses/):", linie]
    aus += [f"- {o}" for o in ohne] + [""]
open(TEXTE, "w", encoding="utf-8", newline="\n").write("\n".join(aus))
print(f"{len(texte)} verschiedene Lizenztexte, {len(ohne)} Pakete ohne Lizenzdatei -> {TEXTE}")
