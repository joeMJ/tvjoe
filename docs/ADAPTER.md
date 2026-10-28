# Adapter: Empfangsquellen und Bibliotheken

[English](ADAPTER.en.md)

## Adapterklassen

tvjoe beschreibt jede Quelle mit einem **Adapter** – einer kleinen TOML-Datei. Es gibt zwei
Klassen:

| Klasse (`class`) | Wofür | Beispiele | Ordner |
|---|---|---|---|
| `television` – Fernsehen | lineares Fernsehen mit Senderliste und Programm | Internet-TV (M3U), SAT>IP-Empfänger, DVB-Karte, HDHomeRun | `television/` |
| `library` – Bibliothek | Mediatheken und Medienserver mit Filmen, Serien, Folgen | ÖR-Mediathek, Jellyfin, Plex | `library/` |

Die mitgelieferten Adapter liegen im Repository unter
[`data/adapter/television/`](../data/adapter/television) und
[`data/adapter/library/`](../data/adapter/library) und sind in tvjoe eingebaut. Ein Adapter
nutzt immer einen **Treiber**, der im Programm steckt; ein Adapter allein kann keinen neuen
Übertragungsweg hinzufügen, aber jedes weitere Gerät oder jeden weiteren Dienst beschreiben, den
ein vorhandener Treiber bedienen kann – ohne zu programmieren.

## Klasse Fernsehen (`television`)

tvjoe kann Fernsehen aus mehreren Quellen gleichzeitig anbieten: Internet-TV, SAT>IP-Empfänger,
DVB-Karten, HDHomeRun und Multicast-IPTV. Jede Quelle hat einen eigenen Knopf in der Kopfleiste,
eine eigene Senderliste mit eigener Sortierung und ein eigenes Programm. Aufnehmen von einer
Quelle geht auch, während eine andere läuft.

Eingerichtet wird unter **Einstellungen → Empfang**; „+“ fügt eine Quelle hinzu.

> [!NOTE]
> Getestet ist nur die **EyeTV Netstream 4Sat** (SAT>IP) und Internet-TV. Alle anderen Quellen
> sind experimentell: Es stand keine Hardware zur Verfügung, DVB-Karten sind nur mit dem
> virtuellen Gerät `dvb_vidtv` des Linux-Kernels geprüft. Rückmeldungen mit echten Geräten sind
> willkommen – über [Fehler melden / Wunsch](https://github.com/joeMJ/tvjoe/issues).

### Begriffe

- **Treiber:** der Teil des Programms, der einen Übertragungsweg beherrscht (z. B. SAT>IP über
  RTSP/RTP). Treiber sind fest eingebaut.
- **Beschreibung (Adapter):** eine kleine TOML-Datei, die sagt, welcher Treiber ein Gerät bedient
  und welche Eigenheiten es hat (Empfangsarten, Zahl der Tuner, Satellit, Senderliste vom Gerät).
  Beschreibungen sind reiner Text – für ein weiteres Gerät mit einem vorhandenen Treiber muss
  niemand programmieren.
- **Empfangsquelle:** eine eingerichtete Instanz einer Beschreibung, z. B. „SAT>IP nach Norm“ mit
  der Adresse deines Empfängers. Dieselbe Beschreibung lässt sich mehrfach verwenden.

### Treiber

| Treiber (`driver`) | Weg | Plattform | Stand |
|---|---|---|---|
| `iptv` | M3U-Listen mit HLS-, HTTP-, `rtp://`- oder `udp://`-Adressen | Linux, Windows | getestet (HLS) |
| `satip` | SAT>IP: RTSP zur Steuerung, RTP über UDP für den Datenstrom | Linux, Windows | getestet mit Netstream 4Sat |
| `linux-dvb` | DVB-Karte (USB, PCI) über `/dev/dvb` | nur Linux | experimentell, geprüft mit `dvb_vidtv` |
| `hdhomerun` | HDHomeRun-Netzwerktuner: Senderliste und Streams über HTTP | Linux, Windows | ungetestet |

Unter Windows gibt es noch keine DVB-Karten (geplant über BDA); eine Fassung für den Mac gibt es
noch nicht.

### Mitgelieferte Beschreibungen

| Beschreibung | Treiber | Empfangsarten | Senderliste | Programm |
|---|---|---|---|---|
| Internet-TV (M3U) | `iptv` | – | M3U-Liste | XMLTV |
| Multicast-IPTV (z. B. MagentaTV) | `iptv` | – | M3U-Liste des Anbieters | XMLTV |
| EyeTV Netstream 4Sat | `satip` | DVB-S/S2 | Suchlauf Astra 19,2° Ost | Signal und/oder XMLTV |
| SAT>IP nach Norm (Satellit) | `satip` | DVB-S/S2 | Suchlauf Astra 19,2° Ost | Signal und/oder XMLTV |
| Digital Devices Octopus NET | `satip` | DVB-S/S2, DVB-T/T2, DVB-C | Suchlauf (Satellit oder Frequenzplan) | Signal und/oder XMLTV |
| Telestar Digibit | `satip` | DVB-S/S2 | Suchlauf Astra 19,2° Ost | Signal und/oder XMLTV |
| Kathrein EXIP | `satip` | DVB-S/S2 | Suchlauf Astra 19,2° Ost | Signal und/oder XMLTV |
| FRITZ!Box Cable | `satip` | DVB-C | von der Box (M3U) | XMLTV |
| HDHomeRun | `hdhomerun` | DVB-T/T2, DVB-C | vom Gerät | XMLTV |
| DVB-Karte (Linux) | `linux-dvb` | was die Karte meldet | Suchlauf (Frequenzplan oder Satellit) | Signal und/oder XMLTV |

Der Suchlauf über Satellit beginnt auf einem Starttransponder und liest von dort die
Netzwerktabelle mit allen Transpondern. Für Antenne und Kabel gibt es eingebaute Frequenzpläne:
**DVB-T2 Deutschland** (UHF-Kanäle 21–48) und **DVB-C Deutschland**. Verschlüsselte Sender werden
ausgelassen; ein erneuter Suchlauf behält Nummern und eigene Namen.

### Einstellungen je Empfangsquelle

Auf der Seite einer Quelle unter **Einstellungen → Empfang**:

- **Ein/aus** – ausgeschaltete Quellen verschwinden aus der Kopfleiste.
- **Gerät** – Adresse des Empfängers bzw. die DVB-Karte; dazu der Suchlauf oder das Laden der
  Senderliste vom Gerät.
- **Senderliste** – eigene Liste mit eigener Sortierung (Sender verwalten).
- **Programm** – eine der unter **Empfang** einmal angelegten XMLTV-Quellen, das Programm aus dem
  Signal (EIT, wenn das Gerät es liefert) oder beides; XMLTV ist nie Pflicht.
- **Senderlogos** – je Quelle ein- und ausschaltbar.
- **Aufnahmen** – Grenze für gleichzeitige Aufnahmen (z. B. nach der Zahl der Tuner).

### Eigene Beschreibung anlegen

Eigene Beschreibungen liegen als `.toml`-Datei im Ordner `adapter/television/` der
Konfiguration (Dateien direkt in `adapter/`, wie bis 0.1.4, gelten weiter als Fernsehen):

- Linux: `~/.config/tvjoe/adapter/television/`
- Windows: `%APPDATA%\tvjoe\adapter\television\`

Sie erscheinen nach dem nächsten Öffnen der Einstellungen unter „+“. Eine eigene Datei mit derselben
Kennung (`id`) wie eine mitgelieferte ersetzt diese. Eine Datei mit Fehlern wird übergangen; den
Grund nennt tvjoe nur mit `TVJOE_TRACE=1` auf der Kommandozeile.

#### Beispiel: SAT>IP-Empfänger für Astra 19,2° Ost

```toml
class = "television"
id = "mein-satip"
name = "Mein SAT>IP-Empfänger"
driver = "satip"
systems = ["dvbs", "dvbs2"]
tuners = 2
signal_epg = true

[satellite]
name = "Astra 19,2° Ost"
orbital = 192
east = true
# Starttransponder: sendet die Netzwerktabelle mit allen Transpondern.
seed = { freq = 11494, pol = "h", dvbs2 = true, mtype = "8psk", sr = 22000, fec = "23" }
```

#### Beispiel: Gerät mit eigener Senderliste (wie die FRITZ!Box)

```toml
class = "television"
id = "mein-kabel"
name = "Mein Kabel-SAT>IP"
driver = "satip"
systems = ["dvbc"]
tuners = 4
signal_epg = false
channel_list = ["http://{host}/dvb/m3u/tvhd.m3u"]
```

#### Felder

| Feld | Pflicht | Bedeutung |
|---|---|---|
| `class` | empfohlen | `television` (fehlt es, gilt der Ordner) |
| `id` | ja | Kennung aus Buchstaben, Ziffern und Bindestrich |
| `name` | ja | Name in den Einstellungen |
| `driver` | ja | `iptv`, `satip`, `linux-dvb` oder `hdhomerun` |
| `systems` | nein | Empfangsarten: `dvbs`, `dvbs2`, `dvbt`, `dvbt2`, `dvbc` |
| `tuners` | nein | Zahl der Tuner laut Hersteller (derzeit nur Angabe; die Aufnahmegrenze stellt man je Quelle ein) |
| `signal_epg` | nein | Das Gerät liefert das Programm mit dem Signal (EIT); Vorgabe `false` |
| `channel_list` | nein | Senderliste vom Gerät statt Suchlauf: Adressen von M3U-Listen; `{host}` steht für die eingetragene Adresse des Geräts |
| `rtp_tcp` | nein | RTP über TCP; tvjoe kann nur `false` (Vorgabe) |
| `connection_per_request` | nein | eine RTSP-Verbindung je Anfrage; tvjoe kann nur `true` (Vorgabe) |
| `[satellite]` | nein | Satellit für den Suchlauf (siehe unten) |

Abschnitt `[satellite]`:

| Feld | Bedeutung |
|---|---|
| `name` | Anzeigename, z. B. „Astra 19,2° Ost“ |
| `orbital` | Orbitalposition in Zehntelgrad (192 = 19,2°) |
| `east` | `true` für Ost, `false` für West |
| `seed` | Starttransponder: `freq` (MHz), `pol` (`h`/`v`), `dvbs2` (`true`/`false`), `mtype` (`qpsk`, `8psk`, `16apsk`, `32apsk`), `sr` (Symbolrate in kS/s), `fec` (`12`, `23`, `34`, `35`, `45`, `56`, `78`, `89`, `910`) |

Für `satip` muss mindestens eines vorhanden sein: `[satellite]`, `channel_list` oder eine
Empfangsart für Antenne bzw. Kabel (dann Suchlauf über einen Frequenzplan). Unbekannte Felder
lehnt tvjoe ab.

## Klasse Bibliothek (`library`)

Bibliotheken sind Mediatheken und Medienserver: Filme, Serien mit Staffeln und Folgen,
Sammlungen. Abspielen, Weitersehen, Herunterladen in die Ablage und der Schalter „Geladen“
funktionieren für alle gleich.

Eingerichtet wird unter **Einstellungen → Mediatheken**: je Bibliothek eine Zeile mit Schalter
und eigener Seite (Name in der Kopfleiste, Einstellungen des Typs); „+“ fügt eine weitere hinzu.
Mehrere Jellyfin- bzw. Plex-Server sind möglich, die ÖR-Mediathek gibt es einmal.

### Treiber

| Treiber (`driver`) | Quelle | Anmeldung | Stand |
|---|---|---|---|
| `oer` | Filmliste von [MediathekView](https://mediathekview.de/), Videos direkt von den Sendern | keine | getestet |
| `jellyfin` | eigener Jellyfin-Server | Quick Connect | getestet |
| `plex` | eigener Plex Media Server | PIN über plex.tv | experimentell, getestet mit Filmen |

### Mitgelieferte Beschreibungen

| Beschreibung | Treiber | Anmeldung | Server-Adresse |
|---|---|---|---|
| ÖR-Mediathek | `oer` | keine | nein |
| Jellyfin | `jellyfin` | Quick Connect | ja |
| Plex | `plex` | PIN (plex.tv/link) | ja |

### Beispiel

```toml
class = "library"
id = "jellyfin-keller"
name = "Jellyfin im Keller"
driver = "jellyfin"
login = "quick-connect"
server = true
```

### Felder

| Feld | Pflicht | Bedeutung |
|---|---|---|
| `class` | ja | `library` |
| `id` | ja | Kennung aus Buchstaben, Ziffern und Bindestrich |
| `name` | ja | Name in den Einstellungen |
| `driver` | ja | `oer`, `jellyfin` oder `plex` |
| `login` | ja | `none` (keine Anmeldung), `quick-connect` (Jellyfin) oder `pin` (Plex) |
| `server` | nein | `true`, wenn die Adresse eines Servers einzutragen ist; Vorgabe `false` |
| `[detail]` | nein | zusätzliche Angaben in der Filminfo, siehe unten |

Eigene Beschreibungen gehören in den Ordner `adapter/library/` der Konfiguration
(`~/.config/tvjoe/adapter/library/` bzw. `%APPDATA%\tvjoe\adapter\library\`).

### Zusätzliche Angaben (`[detail]`)

Was die Filminfo und die Serienseite über die Beschreibung hinaus zeigen (Video, Ton, Untertitel,
Freigabe, Bewertungen, Regie, Besetzung, Links), steht nicht im Programm, sondern im Abschnitt
`[detail]` des Adapters. Ändert ein Server sein Format oder soll eine Angabe dazukommen, genügt
eine Änderung der TOML-Datei (eigene Fassung im Ordner `adapter/library/`, gleiche `id`).

```toml
[detail]
request = "/library/metadata/{id}?includeGuids=1"   # Anfrage beim Server
root = "MediaContainer.Metadata[0]"                  # Wurzel im Ergebnis (leer = alles)

[[detail.field]]                                     # Zeile „Beschriftung – Wert“
label = "Ton"
label_en = "Audio"
value = "Media[0].Part[0].Stream[streamType=2].displayTitle"

[[detail.people]]                                    # Personen mit Rolle und Foto
label = "Besetzung"
list = "Role"
name = "tag"
role = "role"
photo = "{server}/photo/:/transcode?width=160&height=240&url={thumb|url}&X-Plex-Token={token}"
max = 30

[[detail.link]]                                      # Knopf, öffnet die Seite im Browser
label = "IMDb"
url = "https://www.imdb.com/title/{Guid[id^=imdb://].id|strip:imdb://}/"
```

**Pfadsprache** für Fundstellen: Ebenen mit Punkt (`Media.Part`), `[3]` ein Eintrag, `[]` alle,
`[Feld=Wert]` Filter, `[Feld^=Anfang]` Filter nach Textanfang. Ist eine Ebene eine Liste, gilt der
nächste Name für jeden Eintrag; mehrere Treffer ergeben eine Liste.

**Platzhalter** in `request`, `photo` und `url`: `{Pfad}`, `{Pfad|strip:Anfang}` (Anfang
entfernen), `{Pfad|url}` (für Adressen kodiert) sowie `{server}`, `{token}`, `{id}`, `{user_id}`.
Fehlt ein Wert, entfällt der Knopf bzw. das Foto.

| Abschnitt | Feld | Bedeutung |
|---|---|---|
| `[[detail.field]]` | `label`, `label_en` | Beschriftung deutsch und englisch |
| | `value` | Fundstelle; mehrere Treffer werden verbunden |
| | `join` | Trenner bei mehreren Werten (Vorgabe „ · “) |
| | `digits` | Zahlen runden (deutsch mit Komma) |
| | `suffix`, `strip` | Text anhängen bzw. Anfang entfernen |
| `[[detail.people]]` | `label`, `label_en`, `list`, `name`, `role`, `photo`, `max` | Liste, Name, Rolle, Foto-Adresse, Höchstzahl |
| `[[detail.link]]` | `label`, `url`, `when` | Beschriftung, Adresse, Bedingung (`Pfad=Wert`, z. B. `type=movie`) |

Die Angaben erscheinen in der Reihenfolge der Datei; was der Server nicht liefert, bleibt weg.
Eine falsche Fundstelle lässt die Zeile weg; eine fehlgeschlagene Anfrage nennt tvjoe mit
`TVJOE_TRACE=1`.

## Eigene Adapter beisteuern

1. Eine Beschreibung nach den Feldern oben schreiben und im Ordner `adapter/<klasse>/` der
   eigenen Konfiguration ausprobieren. Fehler in der Datei nennt tvjoe mit `TVJOE_TRACE=1`.
2. Läuft sie mit dem eigenen Gerät bzw. Server, gern als Pull-Request nach
   `data/adapter/television/` bzw. `data/adapter/library/` – mit einer Kommentarzeile oben: Gerät
   bzw. Dienst, Modelle, ob getestet und womit.
3. Für einen neuen Übertragungsweg (eigenes Protokoll) braucht es einen Treiber im Programm; dafür
   bitte ein Issue mit Beschreibung des Geräts eröffnen.

## Grenzen

- Eine Beschreibung kann nur vorhandene Treiber nutzen. Ein neuer Übertragungsweg (z. B. ein
  Gerät mit eigenem Protokoll) braucht Code im Programm.
- Eigene Frequenzpläne (andere Länder) lassen sich noch nicht als Datei hinzufügen.
- DVB-Karten brauchen unter Linux einen Treiber des Kernels, oft auch Firmware, und der Benutzer
  braucht Zugriff auf `/dev/dvb` (Gruppe `video`).
