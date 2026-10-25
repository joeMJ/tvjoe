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
- Mediathek der öffentlich-rechtlichen Sender (Filmliste von MediathekView): Episoden als Serien
  mit Staffeln, Filme, Suche; Fassungen (Untertitel, Original, Audiodeskription) zusammengefasst;
  Pause/Spulen, Weitersehen ab der gemerkten Stelle, nächste Folge automatisch
- Info über das ganze Bild (Taste I) für Mediathek und laufende Sendung
- Optionale Updateprüfung über GitHub-Releases (standardmäßig aus)

## Tasten

| Taste | Wirkung |
|---|---|
| Ziffern, Enter | Sender wählen (Pause einstellbar) |
| ← / →, Bild ↑ / ↓ | Sender wechseln |
| ↑ / ↓, + / −, Mausrad | Lautstärke |
| Q | Qualität eine Stufe höher |
| F, Doppelklick / Esc | Vollbild / Vollbild aus |
| M | Ton aus/an |
| L | Senderliste ein/aus |
| I, Esc | Info über das ganze Bild ein / aus |
| A | Tonspur wechseln |
| G | Programmraster |
| Strg+E | Sender verwalten |
| Strg+, | Einstellungen |
| Strg+Q | Beenden |

Beim Abspielen aus der Mediathek:

| Taste | Wirkung |
|---|---|
| Leertaste | Pause / weiter |
| ← / → | 10 Sekunden zurück / vor |
| Bild ↑ / ↓ | 1 Minute zurück / vor |
| Ziffern, Senderliste | zurück zum Fernsehen |

## Installieren (Ubuntu 24.04)

```bash
curl -fsSL https://raw.githubusercontent.com/joeMJ/tvjoe/main/install.sh | bash
```

Lädt das neueste Release-Paket, prüft die Prüfsumme und installiert es (sudo). Derselbe Befehl
aktualisiert eine vorhandene Installation.

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

Sobald die Mediathek einmal geöffnet wurde, kommen hinzu: höchstens einmal täglich die Filmliste
von `liste.mediathekview.de`, für die Vorschaubilder die Sendungsseiten der Sender und deren
Bilder (im Cache abgelegt) und beim Abspielen die Videos von den Servern der Sender. Auch dabei
wird jeweils die IP-Adresse übermittelt.

## Mediathek und MediathekView

Die Mediathek nutzt die Filmliste von [MediathekView](https://mediathekview.de/), einem
ehrenamtlichen Projekt, das die Mediatheken der öffentlich-rechtlichen Sender verzeichnet. tvjoe
lädt die Liste höchstens einmal täglich; die Videos kommen direkt von den Sendern.
MediathekView finanziert die Server über Spenden – wer die Mediathek in tvjoe nutzt, kann das
Projekt hier unterstützen: [mediathekview.de/spenden](https://mediathekview.de/spenden/).

## Lizenz

Apache-2.0, siehe [LICENSE](LICENSE). Bibliotheken Dritter und ihre Lizenzen:
[THIRD_PARTY.md](THIRD_PARTY.md).

Die Ubuntu-Pakete von libmpv stehen laut deren copyright-Datei unter GPL-3+ (gelinkt gegen
libsmbclient). Für die ausgelieferten tvjoe-Binärpakete gelten diese Bedingungen zusätzlich;
Apache-2.0 ist mit GPL-3 verträglich.
