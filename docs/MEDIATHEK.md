# Mediathek und Jellyfin

## Mediathek der öffentlich-rechtlichen Sender

Der Knopf mit dem Filmstreifen im Quellenumschalter öffnet die Mediathek. Die Filmliste kommt von
[MediathekView](https://mediathekview.de/) und wird höchstens einmal täglich geladen; die Videos
kommen direkt von den Servern der Sender.

<p align="center">
  <img src="screenshots/mediathek.webp" alt="Mediathek: Episoden als Serien mit Vorschaubildern und Senderleiste" width="760">
</p>

- **Episoden, Filme, Suche:** Sendereihen erscheinen als Serien mit Staffeln, Filme einzeln.
  Oben die Sender als Logos zum Filtern; **Neueste** bzw. **A–Z** sortiert (je Reiter gemerkt).
- **Geladen:** Der Schalter oben rechts zeigt in allen Reitern nur Heruntergeladenes, gruppiert wie
  sonst (Serien mit ihren Folgen); dasselbe gibt es bei Jellyfin (Filme, Sammlungen, Serien mit
  Staffeln).
- **Fernsehen anhalten:** Solange Mediathek oder Jellyfin offen ist, ruht das Fernsehen; beim
  Schließen läuft der Sender weiter. Abschaltbar unter **Einstellungen → Bedienung**.
- **Fassungen:** Untertitel, Originalton und Audiodeskription eines Beitrags sind zu einer Kachel
  zusammengefasst und lassen sich auf der Seite des Beitrags wählen.
- **Abspielen:** Pause, Spulen, Tonspur (A); tvjoe merkt sich die Stelle und spielt beim nächsten
  Mal dort weiter. Bei Serien läuft die nächste Folge automatisch.
- **Einstellungen → Mediatheken → ÖR-Mediathek:** Mediathek ein/aus, Sender ausblenden, Audiodeskription
  ausblenden.

Ziffern oder die Senderliste führen zurück zum Fernsehen.

## Jellyfin

Ein eigener Jellyfin-Server lässt sich als zweite Mediathek einbinden.

1. **Einstellungen → Mediatheken → Jellyfin:** einschalten und die Adresse des Servers eintragen.
   Weitere Jellyfin-Server fügt „+“ hinzu, jeder mit eigenem Knopf und eigener Anmeldung.
2. **Anmelden** per Quick Connect: tvjoe zeigt einen Code, den man in der Jellyfin-Weboberfläche
   (oben rechts über das Benutzersymbol → Quick Connect) bestätigt. tvjoe fragt nie nach einem
   Passwort; der Zugangsschlüssel liegt nur für den Nutzer lesbar in
   `~/.config/tvjoe/jellyfin.toml`.
3. Der Knopf mit dem Logo des Servers im Quellenumschalter öffnet **Filme, Sammlungen, Serien und
   Suche**. Die Bibliothek wird zwischengespeichert und im Hintergrund abgeglichen.

- **Wiedergabe:** Die Originaldatei läuft per Hardware-Decoding. Mit einer **Bandbreitengrenze**
  (Einstellungen → Mediatheken → Jellyfin) rechnet der Server auf H.264 um; das kostet dort Rechenzeit.
- **Gesehen-Stand:** Fortschritt und „gesehen“ werden an Jellyfin gemeldet und gelten damit auch
  in anderen Jellyfin-Programmen.
- Bibliotheken lassen sich in den Einstellungen ausblenden.

## Plex (experimentell)

Ein eigener Plex Media Server lässt sich genauso einbinden; Ansicht, Ablage, „Geladen“,
Weitersehen und Bandbreitengrenze funktionieren wie bei Jellyfin.

1. **Einstellungen → Mediatheken → „+“ → Plex:** die Adresse des Servers eintragen, im Heimnetz
   meist `http://<IP-Adresse>:32400`.
2. **Anmelden:** tvjoe holt bei plex.tv einen vierstelligen Code; im Browser **plex.tv/link**
   öffnen, mit dem Plex-Konto anmelden und den Code eingeben. Dabei gehen Konto und IP-Adresse an
   plex.tv. tvjoe fragt nie nach einem Passwort; der Zugangsschlüssel liegt nur für den Nutzer
   lesbar in `~/.config/tvjoe/konten/<kennung>.toml`.
3. Der Knopf mit dem Server-Symbol öffnet **Filme, Sammlungen, Serien und Suche**.

- **Weitersehen:** Plex speichert eine Stelle erst ab etwa einer Minute Spielzeit.
- Getestet mit Plex Media Server 1.43 und einer Filmbibliothek; Serien und Sammlungen folgen der
  Plex-Schnittstelle, sind aber ungetestet.

## Emby (experimentell)

Ein eigener Emby-Server funktioniert wie Jellyfin (gleiche Schnittstelle); Ansicht, Ablage,
„Geladen“, Weitersehen und Bandbreitengrenze sind dieselben.

1. **Einstellungen → Mediatheken → „+“ → Emby:** die Adresse eintragen, im Heimnetz meist
   `http://<IP-Adresse>:8096`.
2. **Anmelden:** Benutzer und Passwort eingeben – oder statt des Passworts einen API-Schlüssel
   (in Emby: Einstellungen → Erweitert → API-Schlüssel), der dann für den genannten Benutzer
   gilt (ohne Namen: der erste Administrator). tvjoe speichert nur den Zugangsschlüssel, nicht
   das Passwort; beim Abmelden mit API-Schlüssel wird er nur vergessen, nicht beim Server
   entwertet.

- Getestet mit Emby Server 4.10 und einer Filmbibliothek; Serien folgen der Schnittstelle von
  Jellyfin, sind aber ungetestet.
