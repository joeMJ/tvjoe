# tvjoe bauen

## Ubuntu

Benötigt: `cargo`, `libgtk-4-dev`, `libadwaita-1-dev`, `libmpv-dev`, `pkg-config`.

```bash
cargo build --release
./target/release/tvjoe --check --dev
```

`--check --dev` prüft die Umgebung (Bibliotheken, Hardware-Decoding).

Paket für die laufende Ubuntu-Version: `tools/build-deb.sh` (für 24.04 und 26.04 jeweils dort
bauen). Die Releases baut GitHub Actions für beide Versionen (`.github/workflows/release.yml`).

## Kommandozeile

```text
tvjoe                                 Oberfläche starten
tvjoe --import liste.m3u [--mit-warnungen]
tvjoe --epg-add URL                   XMLTV-Quelle eintragen
tvjoe --epg-check                     Zuordnung der Sender zum Programmführer prüfen
tvjoe --update-check                  nach neuer Version fragen
```

## Selbstprobe

`TVJOE_SELFTEST=<Ordner> tvjoe --mute` fährt Bedienschritte ab und legt nach jedem Schritt ein
Bild des Fensters ab. Weitere Gruppen über Umgebungsvariablen, siehe
`crates/tvjoe-gui/src/selftest.rs`; die Bilder dieser Dokumentation entstehen mit
`TVJOE_SELFTEST_README=1` und neutralen Daten (`TVJOE_CONFIG_DIR`, `TVJOE_CACHE_DIR`).
