#!/usr/bin/env bash
# Installiert oder aktualisiert tvjoe aus dem neuesten GitHub-Release (Ubuntu, .deb).
# Aufruf: curl -fsSL https://raw.githubusercontent.com/joeMJ/tvjoe/main/install.sh | bash
# Alles steht in main(), damit bash bei „curl | bash“ nicht nur einen Teil liest.
set -euo pipefail

main() {
    local repo="joeMJ/tvjoe"
    # shellcheck disable=SC1091
    . /etc/os-release
    local distro="${ID}${VERSION_ID}"
    local arch
    arch=$(dpkg --print-architecture)

    local tmp
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    # apt liest lokale Pakete als Benutzer _apt: Ordner und Datei lesbar machen.
    chmod 755 "$tmp"

    echo "» Suche das neueste Release für $distro/$arch …"
    local json
    json=$(curl -fsSL -H "Accept: application/vnd.github+json" \
        "https://api.github.com/repos/$repo/releases/latest")
    local url sums
    url=$(printf '%s\n' "$json" | grep -oE "\"browser_download_url\": *\"[^\"]*_${distro}_${arch}\\.deb\"" |
        sed -E 's/.*"(https[^"]*)"/\1/' | head -n1)
    sums=$(printf '%s\n' "$json" | grep -oE '"browser_download_url": *"[^"]*/SHA256SUMS"' |
        sed -E 's/.*"(https[^"]*)"/\1/' | head -n1)
    if [ -z "$url" ] || [ -z "$sums" ]; then
        echo "✖ Im neuesten Release gibt es kein Paket für $distro/$arch." >&2
        exit 1
    fi

    local deb
    deb="$tmp/$(basename "$url")"
    echo "» Lade $(basename "$deb") …"
    curl -fsSL -o "$deb" "$url"
    curl -fsSL -o "$tmp/SHA256SUMS" "$sums"
    chmod 644 "$deb"
    echo "» Prüfe die Prüfsumme …"
    (cd "$tmp" && grep -E " \*?$(basename "$deb")\$" SHA256SUMS | sha256sum -c -)

    echo "» Installiere (fragt nach dem sudo-Passwort) …"
    sudo apt-get install -y "$deb"
    echo "✔ tvjoe ist installiert. Start über das Menü oder mit: tvjoe"
}

main "$@"
