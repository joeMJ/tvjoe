# tvjoe

> **Privates Hobbyprojekt – nicht gepflegt / unmaintained.**

Streaming-Client für Live-TV unter Linux (Rust, GTK4/libadwaita, libmpv). Die Sender kommen aus
eigenen M3U-Listen oder von einem SAT>IP-Empfänger im Heimnetz; tvjoe liefert keine
Senderlisten mit. Ziele: schneller Start, geringe
CPU-Last durch Hardware-Decoding.

## Funktionen

- Senderliste mit Nummern, Logos und laufender Sendung, Bild mit Einblendung (Nummer, Name, Jetzt/Danach)
- Umschalten per Ziffernblock, Pfeiltasten oder Klick; Qualitätsstufen SD/720p/1080p/UHD mit Ausweichen
- Import von M3U aus Datei oder URL mit Vorschau, Prüfung und Erreichbarkeitstest; Teilimport
- Senderverwaltung: Umbenennen, Nummern, Reihenfolge (auch an den Anfang/ans Ende, Ziehen mit
  Mitlaufen am Rand), Aktiv/Favorit, Export als M3U8; inaktive Sender zählen bei den Nummern
  nicht mit, neue Sender aus Import oder Suchlauf kommen inaktiv ans Ende
- SAT-Quelle über einen SAT>IP-Empfänger (z. B. EyeTV Netstream 4Sat, Astra 19,2° Ost):
  Prüfen von Gerät und Empfang, Suchlauf in eine eigene SAT-Senderliste (frei empfangbare
  TV-Sender), Empfangsanzeige in der Einblendung, Programm über die vorhandene XMLTV-Quelle,
  Logos aus TV-Liste, XMLTV und der Kodinerds-Sammlung
- Zeilensprung entfernen (gegen Kammstreifen bei 576i/1080i, abschaltbar)
- Sicherung: Einstellungen und Senderlisten in eine Datei exportieren und wieder importieren
  (ohne Jellyfin-Anmeldung)
- Programmführer aus XMLTV (auch `.xml.gz`), Programmraster
- Mediathek der öffentlich-rechtlichen Sender (Filmliste von MediathekView): Episoden als Serien
  mit Staffeln, Filme, Suche; Fassungen (Untertitel, Original, Audiodeskription) zusammengefasst;
  Pause/Spulen, Weitersehen ab der gemerkten Stelle, nächste Folge automatisch
- Eigener Jellyfin-Server als zweite Mediathek (Anmeldung per Quick Connect, ohne Passwort in
  tvjoe): Filme, Sammlungen, Serien, Suche; Gesehen-Stand mit Jellyfin abgeglichen;
  Wiedergabe der Originaldatei per Hardware-Decoding oder, mit Bandbreitengrenze, vom Server
  umgerechnet
- Quellenumschalter TV | SAT | Mediathek | Jellyfin in der Kopfleiste; Programm, Mediathek und
  Verwaltung als Ebene über dem Bild (Ton läuft weiter) oder bei kleinem Fenster als eigenes Fenster
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

Ist Jellyfin eingeschaltet, spricht tvjoe nur mit dem eingetragenen Server: Abgleich der
Bibliothek (30 s nach dem Start, dann alle 30 Minuten), Plakate, Wiedergabe und Meldung des
Gesehen-Stands. Der Zugangsschlüssel liegt in `~/.config/tvjoe/jellyfin.toml` (nur für den
Nutzer lesbar) und geht nur an diesen Server.

Ist die SAT-Quelle eingeschaltet, spricht tvjoe per RTSP mit dem eingetragenen SAT>IP-Gerät im
Heimnetz und empfängt den Datenstrom per RTP über UDP. Für die Logos der SAT-Sender lädt tvjoe
höchstens einmal pro Woche das Verzeichnis der Kodinerds-Logosammlung über die GitHub-API und
die Logos von `raw.githubusercontent.com` (dabei wird die IP-Adresse übermittelt). Den
Datenstrom gibt tvjoe intern nur über `127.0.0.1` an den Player weiter.

## SAT>IP und Firewall

Der SAT>IP-Empfänger schickt den Datenstrom per UDP an tvjoe. Eine aktive Firewall muss UDP
vom Gerät zulassen, sonst kommt kein Bild. Unter Ubuntu mit ufw erkennt der Reiter „SAT>IP“
beim Prüfen den Fall und bietet an, genau diese Regel einzutragen (nach Rückfrage, das System
fragt nach dem Passwort):

```
sudo ufw allow from <Adresse des Geräts> proto udp comment 'SAT>IP'
```

Erlaubt wird nur UDP von diesem einen Gerät.

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
