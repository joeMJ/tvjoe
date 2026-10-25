# Mediathek und Jellyfin

## Mediathek der öffentlich-rechtlichen Sender

Der Knopf mit dem Filmstreifen im Quellenumschalter öffnet die Mediathek. Die Filmliste kommt von
[MediathekView](https://mediathekview.de/) und wird höchstens einmal täglich geladen; die Videos
kommen direkt von den Servern der Sender.

<p align="center">
  <img src="screenshots/mediathek.webp" alt="Mediathek: Episoden als Serien mit Vorschaubildern und Senderleiste" width="760">
</p>

- **Episoden, Filme, Suche:** Sendereihen erscheinen als Serien mit Staffeln, Filme einzeln.
  Oben die Sender als Logos zum Filtern; **Neueste** bzw. **A–Z** sortiert.
- **Fassungen:** Untertitel, Originalton und Audiodeskription eines Beitrags sind zu einer Kachel
  zusammengefasst und lassen sich auf der Seite des Beitrags wählen.
- **Abspielen:** Pause, Spulen, Tonspur (A); tvjoe merkt sich die Stelle und spielt beim nächsten
  Mal dort weiter. Bei Serien läuft die nächste Folge automatisch.
- **Einstellungen → Mediathek:** Mediathek ein/aus, Sender ausblenden, Audiodeskription
  ausblenden.

Ziffern oder die Senderliste führen zurück zum Fernsehen.

## Jellyfin

Ein eigener Jellyfin-Server lässt sich als zweite Mediathek einbinden.

1. **Einstellungen → Jellyfin:** einschalten und die Adresse des Servers eintragen.
2. **Anmelden** per Quick Connect: tvjoe zeigt einen Code, den man in der Jellyfin-Weboberfläche
   (oben rechts über das Benutzersymbol → Quick Connect) bestätigt. tvjoe fragt nie nach einem
   Passwort; der Zugangsschlüssel liegt nur für den Nutzer lesbar in
   `~/.config/tvjoe/jellyfin.toml`.
3. Der Knopf mit dem Logo des Servers im Quellenumschalter öffnet **Filme, Sammlungen, Serien und
   Suche**. Die Bibliothek wird zwischengespeichert und im Hintergrund abgeglichen.

- **Wiedergabe:** Die Originaldatei läuft per Hardware-Decoding. Mit einer **Bandbreitengrenze**
  (Einstellungen → Jellyfin) rechnet der Server auf H.264 um; das kostet dort Rechenzeit.
- **Gesehen-Stand:** Fortschritt und „gesehen“ werden an Jellyfin gemeldet und gelten damit auch
  in anderen Jellyfin-Programmen.
- Bibliotheken lassen sich in den Einstellungen ausblenden.
