# Adapters: reception sources and libraries

[Deutsch](ADAPTER.md)

## Adapter classes

tvjoe describes every source with an **adapter** – a small TOML file. There are two classes:

| Class (`class`) | Purpose | Examples | Folder |
|---|---|---|---|
| `television` | live TV with channel list and guide | Internet TV (M3U), SAT>IP receivers, DVB card, HDHomeRun | `television/` |
| `library` | media libraries and media servers with films, series, episodes | Mediathek of the public broadcasters, Jellyfin (Plex planned) | `library/` |

The included adapters live in the repository under
[`data/adapter/television/`](../data/adapter/television) and
[`data/adapter/library/`](../data/adapter/library) and are built into tvjoe. An adapter always
uses a **driver** inside the program; an adapter alone cannot add a new transmission path, but it
can describe any further device or service an existing driver can handle – without programming.

## Class television

tvjoe can offer TV from several sources at once: Internet TV, SAT>IP receivers, DVB cards,
HDHomeRun and multicast IPTV. Each source has its own button in the header bar, its own channel
list with its own order and its own guide. Recording from one source also works while another one
is playing.

Set them up under **Settings → Reception**; “+” adds a source.

> [!NOTE]
> Only the **EyeTV Netstream 4Sat** (SAT>IP) and Internet TV are tested. All other sources are
> experimental: no hardware was available, and DVB cards are only tested with the Linux kernel's
> virtual device `dvb_vidtv`. Feedback with real devices is welcome – via
> [Report a bug / request a feature](https://github.com/joeMJ/tvjoe/issues).

### Terms

- **Driver:** the part of the program that handles one transmission path (e.g. SAT>IP over
  RTSP/RTP). Drivers are built in.
- **Description (adapter):** a small TOML file that says which driver serves a device and what is
  special about it (delivery systems, number of tuners, satellite, channel list from the device).
  Descriptions are plain text – adding another device for an existing driver needs no programming.
- **Reception source:** a configured instance of a description, e.g. “SAT>IP nach Norm” with the
  address of your receiver. The same description can be used several times.

### Drivers

| Driver (`driver`) | Path | Platform | Status |
|---|---|---|---|
| `iptv` | M3U lists with HLS, HTTP, `rtp://` or `udp://` addresses | Linux, Windows | tested (HLS) |
| `satip` | SAT>IP: RTSP for control, RTP over UDP for the stream | Linux, Windows | tested with Netstream 4Sat |
| `linux-dvb` | DVB card (USB, PCI) via `/dev/dvb` | Linux only | experimental, tested with `dvb_vidtv` |
| `hdhomerun` | HDHomeRun network tuner: channel list and streams over HTTP | Linux, Windows | untested |

DVB cards are not yet available on Windows (planned via BDA); there is no Mac version yet.

### Included descriptions

The names are shown as below (German) in the program.

| Description | Driver | Delivery systems | Channel list | Guide |
|---|---|---|---|---|
| Internet-TV (M3U) | `iptv` | – | M3U list | XMLTV |
| Multicast-IPTV (e.g. MagentaTV) | `iptv` | – | the provider's M3U list | XMLTV |
| EyeTV Netstream 4Sat | `satip` | DVB-S/S2 | scan Astra 19.2° East | signal and/or XMLTV |
| SAT>IP nach Norm (Satellit) – SAT>IP standard | `satip` | DVB-S/S2 | scan Astra 19.2° East | signal and/or XMLTV |
| Digital Devices Octopus NET | `satip` | DVB-S/S2, DVB-T/T2, DVB-C | scan (satellite or frequency plan) | signal and/or XMLTV |
| Telestar Digibit | `satip` | DVB-S/S2 | scan Astra 19.2° East | signal and/or XMLTV |
| Kathrein EXIP | `satip` | DVB-S/S2 | scan Astra 19.2° East | signal and/or XMLTV |
| FRITZ!Box Cable | `satip` | DVB-C | from the box (M3U) | XMLTV |
| HDHomeRun | `hdhomerun` | DVB-T/T2, DVB-C | from the device | XMLTV |
| DVB-Karte (Linux) – DVB card | `linux-dvb` | whatever the card reports | scan (frequency plan or satellite) | signal and/or XMLTV |

A satellite scan starts on a seed transponder and reads the network table with all transponders
from there. For antenna and cable there are built-in frequency plans: **DVB-T2 Germany** (UHF
channels 21–48) and **DVB-C Germany**. Encrypted channels are skipped; a repeated scan keeps
numbers and your own names.

### Settings per reception source

On the page of a source under **Settings → Reception**:

- **On/off** – disabled sources disappear from the header bar.
- **Device** – address of the receiver or the DVB card; plus the scan or loading the channel list
  from the device.
- **Channel list** – its own list with its own order (Manage channels).
- **Guide** – one of the XMLTV sources defined once under **Reception**, the guide from the signal
  (EIT, if the device delivers it) or both; XMLTV is never required.
- **Channel logos** – on or off per source.
- **Recordings** – limit for simultaneous recordings (e.g. by the number of tuners).

### Writing your own description

Your own descriptions are `.toml` files in the `adapter/television/` folder of the configuration
(files directly in `adapter/`, as up to 0.1.4, still count as television):

- Linux: `~/.config/tvjoe/adapter/television/`
- Windows: `%APPDATA%\tvjoe\adapter\television\`

They appear under “+” the next time the settings are opened. A file with the same `id` as an
included description replaces it. A file with errors is skipped; tvjoe only names the reason with
`TVJOE_TRACE=1` on the command line.

#### Example: SAT>IP receiver for Astra 19.2° East

```toml
class = "television"
id = "my-satip"
name = "My SAT>IP receiver"
driver = "satip"
systems = ["dvbs", "dvbs2"]
tuners = 2
signal_epg = true

[satellite]
name = "Astra 19.2° East"
orbital = 192
east = true
# Seed transponder: carries the network table with all transponders.
seed = { freq = 11494, pol = "h", dvbs2 = true, mtype = "8psk", sr = 22000, fec = "23" }
```

#### Example: device with its own channel list (like the FRITZ!Box)

```toml
class = "television"
id = "my-cable"
name = "My cable SAT>IP"
driver = "satip"
systems = ["dvbc"]
tuners = 4
signal_epg = false
channel_list = ["http://{host}/dvb/m3u/tvhd.m3u"]
```

#### Fields

| Field | Required | Meaning |
|---|---|---|
| `class` | recommended | `television` (if missing, the folder decides) |
| `id` | yes | identifier of letters, digits and hyphens |
| `name` | yes | name in the settings |
| `driver` | yes | `iptv`, `satip`, `linux-dvb` or `hdhomerun` |
| `systems` | no | delivery systems: `dvbs`, `dvbs2`, `dvbt`, `dvbt2`, `dvbc` |
| `tuners` | no | number of tuners per the manufacturer (information only for now; the recording limit is set per source) |
| `signal_epg` | no | the device delivers the guide with the signal (EIT); default `false` |
| `channel_list` | no | channel list from the device instead of a scan: addresses of M3U lists; `{host}` stands for the device address you entered |
| `rtp_tcp` | no | RTP over TCP; tvjoe only supports `false` (default) |
| `connection_per_request` | no | one RTSP connection per request; tvjoe only supports `true` (default) |
| `[satellite]` | no | satellite for the scan (see below) |

Section `[satellite]`:

| Field | Meaning |
|---|---|
| `name` | display name, e.g. “Astra 19.2° East” |
| `orbital` | orbital position in tenths of a degree (192 = 19.2°) |
| `east` | `true` for east, `false` for west |
| `seed` | seed transponder: `freq` (MHz), `pol` (`h`/`v`), `dvbs2` (`true`/`false`), `mtype` (`qpsk`, `8psk`, `16apsk`, `32apsk`), `sr` (symbol rate in kS/s), `fec` (`12`, `23`, `34`, `35`, `45`, `56`, `78`, `89`, `910`) |

For `satip`, at least one of these is required: `[satellite]`, `channel_list` or a delivery system
for antenna or cable (then the scan uses a frequency plan). Unknown fields are rejected.

## Class library

Libraries are media libraries and media servers: films, series with seasons and episodes,
collections. Playback, resume, downloading to storage and the “Downloaded” toggle work the same
for all of them.

> [!NOTE]
> The class is defined; setting libraries up from your own descriptions follows with the next
> expansion (Settings → Media libraries, several servers per type). Until then the Mediathek and
> Jellyfin are set up as before under **Settings → Mediathek** and **→ Jellyfin**.

### Drivers

| Driver (`driver`) | Source | Login | Status |
|---|---|---|---|
| `oer` | film list from [MediathekView](https://mediathekview.de/), videos straight from the broadcasters | none | tested |
| `jellyfin` | your own Jellyfin server | Quick Connect | tested |
| `plex` | your own Plex server | PIN via plex.tv | planned |

### Included descriptions

| Description | Driver | Login | Server address |
|---|---|---|---|
| ÖR-Mediathek | `oer` | none | no |
| Jellyfin | `jellyfin` | Quick Connect | yes |

### Example

```toml
class = "library"
id = "jellyfin-basement"
name = "Jellyfin in the basement"
driver = "jellyfin"
login = "quick-connect"
server = true
```

### Fields

| Field | Required | Meaning |
|---|---|---|
| `class` | yes | `library` |
| `id` | yes | identifier of letters, digits and hyphens |
| `name` | yes | name in the settings |
| `driver` | yes | `oer` or `jellyfin` |
| `login` | yes | `none` (no login) or `quick-connect` (Jellyfin) |
| `server` | no | `true` if a server address has to be entered; default `false` |

Your own descriptions go into the `adapter/library/` folder of the configuration
(`~/.config/tvjoe/adapter/library/` or `%APPDATA%\tvjoe\adapter\library\`).

## Contributing adapters

1. Write a description following the fields above and try it in the `adapter/<class>/` folder of
   your own configuration. tvjoe names errors in the file with `TVJOE_TRACE=1`.
2. Once it works with your device or server, a pull request to `data/adapter/television/` or
   `data/adapter/library/` is welcome – with a comment line at the top: device or service, models,
   whether and how it was tested.
3. A new transmission path (its own protocol) needs a driver in the program; please open an issue
   describing the device.

## Limits

- A description can only use existing drivers. A new transmission path (e.g. a device with its own
  protocol) needs code in the program.
- Your own frequency plans (other countries) cannot be added as files yet.
- On Linux, DVB cards need a kernel driver, often firmware too, and the user needs access to
  `/dev/dvb` (group `video`).
