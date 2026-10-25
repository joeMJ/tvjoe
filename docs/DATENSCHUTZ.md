# Datenschutz und Netzzugriffe

tvjoe sendet keine Nutzungsdaten und hat keine Telemetrie. Bei jedem Zugriff auf einen fremden
Server wird, wie bei jedem Netzzugriff, die eigene IP-Adresse übermittelt.

## Immer

- Die Streams und Logos aus den eigenen Senderlisten.
- Die eingetragenen XMLTV-Quellen (Programm), im eingestellten Abstand.

## Nur wenn eingeschaltet

- **Updateprüfung** (standardmäßig aus): eine Anfrage an die GitHub-API nach dem neuesten Release
  dieses Repositorys.
- **Mediathek:** Sobald die Mediathek einmal geöffnet wurde, höchstens einmal täglich die
  Filmliste von `liste.mediathekview.de`; für die Vorschaubilder die Sendungsseiten der Sender und
  deren Bilder (im Zwischenspeicher abgelegt); beim Abspielen die Videos von den Servern der Sender.
- **Jellyfin:** nur der eingetragene Server – Abgleich der Bibliothek (30 s nach dem Start, dann
  alle 30 Minuten), Plakate, Wiedergabe und Meldung des Gesehen-Stands. Der Zugangsschlüssel liegt
  in `~/.config/tvjoe/jellyfin.toml` (nur für den Nutzer lesbar) und geht nur an diesen Server.
  Für das Logo im Quellenumschalter lädt tvjoe das Symbol der Jellyfin-Weboberfläche.
- **SAT-Quelle:** RTSP (TCP 554) zum eingetragenen SAT>IP-Gerät im Heimnetz und der Datenstrom
  per RTP über UDP. Den Datenstrom gibt tvjoe intern nur über `127.0.0.1` an den Player weiter.
  Für die Logos der SAT-Sender höchstens einmal pro Woche das Verzeichnis der Kodinerds-
  Logosammlung über die GitHub-API und die Logos von `raw.githubusercontent.com`.

## Fehler melden

Der Eintrag **Einstellungen → Updates → „Fehler melden / Wunsch“** öffnet nur die Issue-Seite bei
GitHub im Browser; tvjoe selbst überträgt dabei nichts.

## Gespeicherte Daten

- `~/.config/tvjoe/`: Einstellungen (`settings.toml`), TV-Senderliste (`library.toml`),
  SAT-Senderliste (`satip.toml`), Jellyfin-Anmeldung (`jellyfin.toml`).
- `~/.cache/tvjoe/`: Logos, Programm, Filmliste der Mediathek mit Weiterseh-Positionen,
  Zwischenspeicher der Jellyfin-Bibliothek, eigene Symbole.
