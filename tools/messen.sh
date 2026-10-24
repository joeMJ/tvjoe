#!/usr/bin/env bash
# Misst Startzeit (bis Fenster und erstes Bild), CPU-Last und – falls ein Akku da ist – die
# Leistungsaufnahme während der Wiedergabe. Die Wiedergabe läuft stumm und endet von selbst.
#
# Aufruf: tools/messen.sh [SEKUNDEN] [KANALNUMMER]
#   SEKUNDEN     Messdauer nach 10 s Anlaufzeit (Standard 30)
#   KANALNUMMER  Sender aus der Bibliothek (Standard: letzter bzw. erster)
# Umgebung wie bei tvjoe (z. B. TVJOE_CONFIG_DIR für eine Testbibliothek).
set -euo pipefail

dauer=${1:-30}
kanal=${2:-}
anlauf=10
dir="$(cd "$(dirname "$0")/.." && pwd)"
# Eigener Build bevorzugt, sonst das installierte Paket.
bin="$dir/target/release/tvjoe"
[ -x "$bin" ] || bin="$(command -v tvjoe || true)"
[ -n "$bin" ] && [ -x "$bin" ] || { echo "Weder Build noch Paket gefunden (cargo build --release oder .deb installieren)" >&2; exit 1; }
echo "Messe mit: $bin"

log="$(mktemp -t tvjoe-messung.XXXXXX)"
args=(--mute --quit-after $((anlauf + dauer + 2)))
[ -n "$kanal" ] && args+=(--channel "$kanal")

TVJOE_TRACE=1 "$bin" "${args[@]}" 2>"$log" &
pid=$!

ticks() { awk '{print $14 + $15}' "/proc/$pid/stat" 2>/dev/null || echo 0; }
akku="$(ls -d /sys/class/power_supply/BAT* 2>/dev/null | head -1 || true)"

sleep "$anlauf"
t1=$(ticks)
leistung=()
for _ in $(seq "$dauer"); do
    if [ -n "$akku" ] && [ -r "$akku/power_now" ]; then
        leistung+=("$(cat "$akku/power_now")")
    fi
    sleep 1
done
t2=$(ticks)
wait "$pid" || true

hz=$(getconf CLK_TCK)
echo "== Zeitmarken"
grep -E 'Oberfläche gestartet|Fenster angezeigt|erstes Bild|hwdec=' "$log" || cat "$log"
echo "== CPU"
awk -v a="$t1" -v b="$t2" -v hz="$hz" -v d="$dauer" -v n="$(nproc)" 'BEGIN {
    p = (b - a) / hz / d * 100
    printf "tvjoe: %.1f %% eines Kerns (%.2f %% der %d Kerne) über %d s\n", p, p / n, n, d }'
if [ "${#leistung[@]}" -gt 0 ]; then
    echo "== Akku (Entladeleistung des ganzen Geräts)"
    printf '%s\n' "${leistung[@]}" | awk '{ s += $1 } END { printf "Mittel: %.2f W über %d Messungen\n", s / NR / 1e6, NR }'
fi
echo "Protokoll: $log"
