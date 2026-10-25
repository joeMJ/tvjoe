# Erste Schritte

## Senderliste übernehmen (M3U)

1. **Sender verwalten** öffnen (Stift-Knopf oben links oder Strg+E).
2. **Datei importieren …** oder **Von URL …** wählen.
3. Die Vorschau prüft jeden Eintrag (Adresse, Erreichbarkeit, Qualitätsstufen) und zeigt
   Warnungen. Einzelne Sender oder ganze Gruppen lassen sich ab- oder zuwählen.
4. **Übernehmen** legt die Sender an. Mehrere Einträge desselben Senders in verschiedenen
   Qualitäten werden zu einem Sender mit Stufen zusammengefasst.

Bei der ersten Übernahme in eine leere Liste sind alle Sender aktiv und nummeriert. Später neu
hinzukommende Sender landen **inaktiv und ohne Nummer am Ende**, damit die eigene Sortierung
erhalten bleibt; aktivieren und einsortieren lassen sie sich in der Verwaltung.

Läuft gerade die SAT-Quelle, schaltet tvjoe vor der Übernahme nach Rückfrage auf TV um: eine
M3U-Liste gehört immer zur TV-Senderliste.

## Programm eintragen (XMLTV)

In der Verwaltung **EPG-Quellen …**: eine XMLTV-Datei oder -Adresse eintragen (auch `.xml.gz`).
tvjoe ordnet die Sender über die Kennung aus der M3U oder über den Namen zu; die Spalte **EPG**
der Verwaltung zeigt, wie (automatisch, unscharf, manuell). Über das Kontextmenü
**EPG-Zuordnung …** lässt sich ein Sender von Hand zuordnen. Wie oft das Programm neu geladen
wird, steht unter **Einstellungen → Programm**.

## Senderliste sortieren

In der Verwaltung:

- **Ziehen und Ablegen** einer Zeile verschiebt den Sender; am oberen und unteren Rand läuft die
  Liste beim Ziehen mit.
- **Rechte Maustaste** auf einen oder mehrere markierte Sender: **An den Anfang verschieben**,
  **Nach oben**, **Nach unten**, **Ans Ende verschieben**, Aktivieren/Deaktivieren, Favorit,
  Umbenennen, Nummer ändern, Exportieren, Löschen.
- Markieren: Strg- bzw. Umschalt-Klick für mehrere Sender; ein erneuter Klick auf einen
  markierten Sender hebt nur dessen Auswahl auf, **Esc** hebt die ganze Auswahl auf.
- Die Nummern folgen der Reihenfolge. **Inaktive Sender bekommen keine Nummer** und zählen nicht mit.

Die Senderliste über dem Bild nimmt Verschiebungen per Ziehen ebenfalls an.

## Anzeige einstellen

- **Einstellungen → Senderanzeige:** was die Einblendung zeigt (Nummer, Quellsymbol, Logo,
  Kategorie, Qualität, Jetzt/Danach, Beschreibung). Die Logos lassen sich für Internet- und
  SAT-Sender getrennt abschalten; das gilt auch für Info und Senderliste.
- **Einstellungen → Qualität:** gewünschte Stufe, Obergrenze, Verhalten bei Störungen und
  **Zeilensprung entfernen** (gegen Kammstreifen bei Bewegung, z. B. SD-Sender über SAT; wirkt nur
  bei Zeilensprung-Bildern und kostet bei Software-Decoding etwas Rechenzeit).
- **Einstellungen → Bedienung:** Ziffernblock-Pause, Dauer der Einblendung, letzter Sender beim
  Start, Bildschirmsperre, Senderliste beim Start.

## Sichern und zurückholen

**Einstellungen → Bedienung → Sichern:**

- **Exportieren …** schreibt alle Einstellungen, die TV- und die SAT-Senderliste (mit Sortierung,
  eigenen Namen, Favoriten und Programmzuordnung) in eine Datei, z. B. `tvjoe-sicherung.toml`.
- **Importieren …** zeigt, wie viele Sender die Sicherung enthält, und ersetzt nach Rückfrage
  Einstellungen und beide Senderlisten. Die bisherigen Dateien bleiben als `.bak` erhalten.

Nicht enthalten sind die Jellyfin-Anmeldung, Zwischenspeicher (Logos, Programm, Filmliste) und
Weiterseh-Positionen.
