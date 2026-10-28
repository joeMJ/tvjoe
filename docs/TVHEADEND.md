# Tvheadend als Empfangsquelle

tvjoe kann einen [Tvheadend](https://tvheadend.org/)-Server im eigenen Netz als Empfangsquelle
nutzen: die Kanäle kommen von Tvheadend, die Wiedergabe läuft über Tvheadend, das Programm wahlweise
von Tvheadend oder aus XMLTV. Woher Tvheadend die Sender selbst bekommt – DVB-Karte oder -Stick,
SAT>IP-Empfänger, HDHomeRun, IPTV/M3U – spielt für tvjoe keine Rolle.

Experimentell. Getestet mit Tvheadend 4.3 (Paket aus der Quelle des Projekts, Debian 13) mit
IPTV aus einer M3U-Liste und mit Satellit über einen SAT>IP-Empfänger (EyeTV Netstream 4Sat,
Astra 19,2° Ost).

## In Tvheadend vorbereiten

1. **Sender:** Netzwerk und Tuner einrichten, Suchlauf, Dienste auf Kanäle abbilden (Assistent
   von Tvheadend oder Konfiguration → DVB-Eingänge). tvjoe übernimmt die *Kanäle*, nicht die
   Dienste.
2. **Benutzer für tvjoe:** Konfiguration → Benutzer → Zugriffseinträge: neuer Eintrag mit dem
   Benutzernamen, Rechte **Streaming** (Standard genügt) und Zugriff auf die gewünschten Kanäle
   bzw. Kanal-Tags. Unter **Passwörter** ein Passwort für denselben Benutzer anlegen.
   Admin-Rechte braucht tvjoe nicht.
3. **Netz:** tvjoe spricht Tvheadend über HTTP an (Port 9981). Freigabe ins Internet ist nicht
   nötig; im Heimnetz genügt die IP-Adresse.

Tvheadend meldet sich in der Grundeinstellung nur per HTTP-Digest an; tvjoe beherrscht das
(Basic geht ebenfalls, falls Tvheadend darauf gestellt ist).

## In tvjoe eintragen

**Einstellungen → Empfang → „+“ → Tvheadend**, dann auf der Seite der Quelle:

1. **Adresse:** z. B. `192.0.2.5` oder `http://tvheadend.example.org:9981` – ohne Angabe gelten
   `http://` und Port `9981`.
2. **Benutzer** und **Passwort** des Tvheadend-Benutzers.
3. **Prüfen:** meldet sich an und zeigt die Fassung („Verbunden: Tvheadend 4.3-…“).
4. **Kanäle holen:** übernimmt die eingeschalteten Kanäle in Tvheadend-Reihenfolge. Ein erneutes
   Holen behält Nummern, eigene Namen und Zuordnungen; neue Kanäle kommen dazu, weggefallene
   werden als „nicht mehr gefunden“ markiert.

Das Passwort liegt nur für den Nutzer lesbar in `~/.config/tvjoe/konten/tvheadend-<host>.toml`
(Windows: `%APPDATA%\tvjoe\konten\…`). In der Senderliste steht je Sender nur
`tvheadend://<host:port>/<kanal>`. Beim Umschalten holt tvjoe für den Kanal ein zeitlich
begrenztes **Ticket** bei Tvheadend und spielt die Adresse mit diesem Ticket ab – das Passwort
erscheint so in keiner Stream-Adresse.

## Programm (EPG)

Neue Sender bekommen das Programm **von Tvheadend** (`/api/epg/events/grid`). tvjoe holt es im
eingestellten Abstand („Von Tvheadend holen alle … Stunden“, eine Anfrage an den Server, kein
Tuner nötig). Je Sender lässt sich in **Sender verwalten** → EPG zwischen „Von Tvheadend“ und
einem Kanal aus den XMLTV-Quellen der Empfangsquelle wählen; XMLTV-Quellen bleiben zusätzlich
nutzbar.

Woher Tvheadend das Programm hat, entscheidet Tvheadend:

- **Aus dem Signal (DVB, SAT>IP):** Tvheadend sammelt das Programm aus dem Datenstrom
  („OTA EPG“) – dafür belegt es zeitweise einen Tuner.
- **IPTV:** der Strom enthält meist kein Programm. Tvheadend kann XMLTV selbst einlesen, z. B.
  mit einem kleinen Grabber-Skript (Beispiel unten). Danach unter Konfiguration → Kanal/EPG →
  EPG-Grabber-Module das Modul einschalten und unter **EPG-Grabber-Kanäle** die XMLTV-Kanäle
  den Tvheadend-Kanälen zuordnen (gleich lautende Namen ordnet Tvheadend teils selbst zu).

Beispiel für einen Grabber (`/usr/bin/tv_grab_beispiel`, ausführbar; Tvheadend erkennt
`tv_grab_*` beim Start):

```sh
#!/bin/sh
# XMLTV-Grabber für Tvheadend: lädt eine XMLTV-Datei (hier gepackt) und gibt sie aus.
case "$1" in
  --description) echo "Beispiel (XMLTV)"; exit 0 ;;
  --capabilities) echo "baseline"; exit 0 ;;
  --version) echo "1.0"; exit 0 ;;
esac
exec curl -fsSL --max-time 300 https://example.org/epg.xml.gz | gunzip -c
```

Die Adresse der XMLTV-Datei ist ein Beispiel; welche Quelle für deine Sender passt und unter
welchen Bedingungen sie genutzt werden darf, ist deine Entscheidung.

## Verschlüsselte Sender (Pay-TV)

Kann Tvheadend einen Kanal nicht entschlüsseln (kein CI-Modul, keine Karte), meldet es im
eigenen Protokoll „No descrambler“, antwortet tvjoe aber trotzdem mit HTTP 200 und schickt dann
nichts. tvjoe liest deshalb vor dem Abspielen kurz an (höchstens 5 Sekunden) und zeigt dann
„Kein Datenstrom von Tvheadend – verschlüsselter Sender ohne Entschlüsselung (Pay-TV) oder kein
freier Tuner“ statt zu warten. Bei frei empfangbaren Sendern kostet das etwa eine Sekunde beim
Umschalten.

## Tuner teilen

Tvheadend belegt Tuner nicht nur beim Schauen, sondern auch für Suchläufe und das Programm aus
dem Signal. Nutzen andere Geräte denselben SAT>IP-Empfänger direkt (z. B. tvjoe mit der Quelle
„SAT“), Tvheadend in den Tuner-Einstellungen nur einen Teil der Tuner geben (die übrigen dort
ausschalten).

## Fehlersuche

- **„Tvheadend: Benutzer oder Passwort falsch, oder keine Rechte“:** Zugriffseintrag und
  Passwort des Benutzers prüfen (beides muss für denselben Namen angelegt sein).
- **„Tvheadend meldet keine Kanäle“:** In Tvheadend sind noch keine Dienste auf Kanäle
  abgebildet, oder der Benutzer hat keinen Zugriff auf die Kanal-Tags.
- **Sender ohne Programm:** In tvjoe unter Sender verwalten die EPG-Zuordnung prüfen; in
  Tvheadend, ob der Kanal einem EPG-Grabber-Kanal zugeordnet ist.
- Für genaue Angaben: `tvjoe --diagnose` schreibt ein vollständiges Protokoll
  (`~/.cache/tvjoe/diagnose-…log`), Zugangsschlüssel und Tickets bleiben dabei außen vor.
