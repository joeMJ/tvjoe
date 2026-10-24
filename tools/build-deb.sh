#!/usr/bin/env bash
# Baut ein .deb für die laufende Ubuntu-Version. Pakete sind an die glibc der Bauumgebung
# gebunden: für Ubuntu 24.04 und 26.04 jeweils auf der Zielversion bauen.
# Ergebnis: target/deb/tvjoe_<version>_<distro>_<arch>.deb und target/deb/SHA256SUMS
# (Namensschema wie von der Updatefunktion erwartet).
set -euo pipefail
# Paketinhalt mit üblichen Rechten (755/644), unabhängig von der umask der Sitzung.
umask 022

dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$dir"
# Ubuntu legt rustfmt/clippy versioniert ab; für cargo selbst nicht nötig.
for d in /usr/lib/rust-*/bin; do [ -d "$d" ] && PATH="$d:$PATH"; done

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
. /etc/os-release
distro="${ID}${VERSION_ID}"
arch=$(dpkg --print-architecture)

cargo build --release --locked

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
# mktemp legt 700 an; das Wurzelverzeichnis des Pakets muss 755 sein.
chmod 755 "$stage"
install -Dm755 target/release/tvjoe "$stage/usr/lib/tvjoe/tvjoe"
install -Dm755 target/release/tvjoe-gui "$stage/usr/lib/tvjoe/tvjoe-gui"
mkdir -p "$stage/usr/bin"
ln -s ../lib/tvjoe/tvjoe "$stage/usr/bin/tvjoe"
install -Dm644 data/io.github.joeMJ.tvjoe.desktop "$stage/usr/share/applications/io.github.joeMJ.tvjoe.desktop"
install -Dm644 data/io.github.joeMJ.tvjoe.svg "$stage/usr/share/icons/hicolor/scalable/apps/io.github.joeMJ.tvjoe.svg"
install -Dm644 LICENSE "$stage/usr/share/doc/tvjoe/copyright"

size=$(du -sk "$stage/usr" | cut -f1)
mkdir -p "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<EOF
Package: tvjoe
Version: $version
Architecture: $arch
Maintainer: joeMJ <1265358+joeMJ@users.noreply.github.com>
Installed-Size: $size
Depends: libgtk-4-1 (>= 4.12), libadwaita-1-0 (>= 1.5), libmpv2, libegl1, libc6
Section: video
Priority: optional
Homepage: https://github.com/joeMJ/tvjoe
Description: Live-TV aus eigenen M3U-Listen
 Schneller Streaming-Client (GTK4/libadwaita, libmpv) mit Senderverwaltung,
 Ziffernblock-Anwahl, Senderlogos und Programmführer aus XMLTV.
EOF

out=target/deb
mkdir -p "$out"
deb="$out/tvjoe_${version}_${distro}_${arch}.deb"
dpkg-deb --root-owner-group --build "$stage" "$deb" >/dev/null
(cd "$out" && sha256sum tvjoe_*.deb > SHA256SUMS)
echo "$deb"
dpkg-deb --info "$deb" | sed -n '/Package:/,$p'
dpkg-deb --contents "$deb"
