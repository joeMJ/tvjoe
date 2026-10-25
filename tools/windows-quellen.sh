#!/usr/bin/env bash
# windows-quellen.sh – lädt die Quellpakete aller in der Windows-MSI mitgelieferten MSYS2-Pakete
# (Liste tvjoe-<version>-windows-quellen.tsv aus tools/build-windows.ps1) und packt sie unverändert
# in tvjoe-<version>-windows-quellen.tar samt .sha256. Das Archiv kommt mit der MSI ins Release:
# Quelltext der GPL/LGPL-Teile am selben Ort wie die Programme (nur auf Gitea, nie öffentlich).
#
#   tools/windows-quellen.sh <version> <ordner-mit-tsv>
set -euo pipefail

VERSION="${1:?Version fehlt, z. B. 0.0.10}"
DIR="${2:?Ordner mit der Quellenliste fehlt}"
TSV="$DIR/tvjoe-$VERSION-windows-quellen.tsv"
NAME="tvjoe-$VERSION-windows-quellen"
fail() { echo "✖ $*" >&2; exit 1; }

[ -f "$TSV" ] || fail "Quellenliste fehlt: $TSV"
WORK="$DIR/$NAME"
rm -rf "$WORK"
mkdir -p "$WORK"
cp "$TSV" "$WORK/QUELLEN.tsv"

n=0
while IFS=$'\t' read -r paket version basis url; do
    [ -n "$url" ] || continue
    datei="$(basename "$url")"
    # Mehrere Pakete teilen sich ein Quellpaket (gleiche Basis): nur einmal laden.
    [ -f "$WORK/$datei" ] && continue
    echo "» $datei"
    curl -fsSL --retry 3 -o "$WORK/$datei" "$url" || fail "Quellpaket nicht ladbar: $url (Paket $paket $version)"
    n=$((n + 1))
done < <(tr -d '\r' < "$TSV")
echo "» $n Quellpakete geladen"

tar -C "$DIR" -cf "$DIR/$NAME.tar" "$NAME"
( cd "$DIR" && sha256sum "$NAME.tar" > "$NAME.tar.sha256" )
rm -rf "$WORK"
echo "✔ $DIR/$NAME.tar ($(du -h "$DIR/$NAME.tar" | cut -f1)), $(cut -d' ' -f1 "$DIR/$NAME.tar.sha256")"
