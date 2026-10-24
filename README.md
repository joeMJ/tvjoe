# tvjoe

> **Privates Hobbyprojekt – nicht gepflegt / unmaintained.**

Streaming-Client für Live-TV unter Linux (Rust, GTK4/libadwaita, libmpv). Die Sender kommen aus
eigenen M3U-Listen; tvjoe liefert keine Senderlisten mit. Ziele: schneller Start, geringe
CPU-Last durch Hardware-Decoding.

## Funktionen

- Senderliste mit Nummern, Logos und laufender Sendung, Bild mit Einblendung (Nummer, Name, Jetzt/Danach)
- Umschalten per Ziffernblock, Pfeiltasten oder Klick; Qualitätsstufen SD/720p/1080p/UHD mit Ausweichen
- Import von M3U aus Datei oder URL mit Vorschau, Prüfung und Erreichbarkeitstest; Teilimport
- Senderverwaltung: Umbenennen, Nummern, Reihenfolge, Aktiv/Favorit, Export als M3U8
- Programmführer aus XMLTV (auch `.xml.gz`), Programmraster
- Optionale Updateprüfung über GitHub-Releases (standardmäßig aus)

## Tasten

| Taste | Wirkung |
|---|---|
| Ziffern, Enter | Sender wählen (Pause einstellbar) |
| ↑ / ↓, Bild ↑ / ↓ | Sender wechseln |
| ← / →, Mausrad | Lautstärke |
| Q | Qualität eine Stufe höher |
| F, Doppelklick / Esc | Vollbild / Vollbild aus |
| M | Ton aus/an |
| L | Senderliste ein/aus |
| I | Info |
| G | Programmraster |
| Strg+E | Sender verwalten |
| Strg+, | Einstellungen |
| Strg+Q | Beenden |

## Bauen

Benötigt (Ubuntu): `cargo`, `libgtk-4-dev`, `libadwaita-1-dev`, `libmpv-dev`, `pkg-config`.

```bash
cargo build --release
./target/release/tvjoe --check --dev
```

Paket für die laufende Ubuntu-Version: `tools/build-deb.sh` (für 24.04 und 26.04 jeweils dort bauen).

## Kommandozeile

```text
tvjoe                                 Oberfläche starten
tvjoe --import liste.m3u [--mit-warnungen]
tvjoe --epg-add URL                   XMLTV-Quelle eintragen
tvjoe --epg-check                     Zuordnung der Sender zum Programmführer prüfen
tvjoe --update-check                  nach neuer Version fragen
```

## Datenschutz

tvjoe sendet keine Nutzungsdaten. Netzzugriffe: die Streams und Logos aus den eigenen Listen,
die eingetragenen EPG-Quellen und – nur wenn eingeschaltet – eine Anfrage an die GitHub-API
zur Updateprüfung (dabei wird die IP-Adresse übermittelt).

## Lizenz

Apache-2.0, siehe [LICENSE](LICENSE). libmpv ist unter Ubuntu mit GPL-Funktionen gebaut; für
ausgelieferte Binärpakete gelten deren Bedingungen zusätzlich.
