# tvjoe installieren

## Installation (Ubuntu)

Für Ubuntu 26.04 und 24.04 gibt es je ein eigenes Paket (`.deb`). Am einfachsten installiert
das Skript das passende Paket aus dem neuesten Release:

```bash
curl -fsSL https://raw.githubusercontent.com/joeMJ/tvjoe/main/install.sh | bash
```

Das Skript erkennt Ubuntu-Version und Architektur, lädt das Paket und die Datei `SHA256SUMS`,
prüft die Prüfsumme und installiert mit `sudo apt install`. Derselbe Befehl aktualisiert eine
vorhandene Installation.

Von Hand: Auf der [Release-Seite](https://github.com/joeMJ/tvjoe/releases/latest) das Paket für
die eigene Ubuntu-Version laden (`tvjoe_<Version>_ubuntu26.04_amd64.deb` bzw. `…ubuntu24.04…`),
die Prüfsumme mit `SHA256SUMS` vergleichen und installieren:

```bash
sudo apt install ./tvjoe_<Version>_ubuntu26.04_amd64.deb
```

tvjoe erscheint danach im Anwendungsmenü; im Terminal startet es mit `tvjoe`.

Entfernen:

```bash
sudo apt remove tvjoe
```

Einstellungen und Senderlisten bleiben dabei in `~/.config/tvjoe/`, Zwischenspeicher (Logos,
Programm, Filmliste) in `~/.cache/tvjoe/`.

### Updates

Unter **Einstellungen → Updates** lässt sich eine Prüfung auf neue Versionen einschalten
(standardmäßig aus) oder einmal von Hand auslösen. Aktualisiert wird mit dem Installationsbefehl
oben.

## Installation (Windows)

In Vorbereitung.
