# Reception sources and adapters

[Deutsch](ADAPTER.md)

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

## Terms

- **Driver:** the part of the program that handles one transmission path (e.g. SAT>IP over
  RTSP/RTP). Drivers are built in.
- **Description (adapter):** a small TOML file that says which driver serves a device and what is
  special about it (delivery systems, number of tuners, satellite, channel list from the device).
  Descriptions are plain text – adding another device for an existing driver needs no programming.
- **Reception source:** a configured instance of a description, e.g. “SAT>IP nach Norm” with the
  address of your receiver. The same description can be used several times.

## Drivers

| Driver (`driver`) | Path | Platform | Status |
|---|---|---|---|
| `iptv` | M3U lists with HLS, HTTP, `rtp://` or `udp://` addresses | Linux, Windows | tested (HLS) |
| `satip` | SAT>IP: RTSP for control, RTP over UDP for the stream | Linux, Windows | tested with Netstream 4Sat |
| `linux-dvb` | DVB card (USB, PCI) via `/dev/dvb` | Linux only | experimental, tested with `dvb_vidtv` |
| `hdhomerun` | HDHomeRun network tuner: channel list and streams over HTTP | Linux, Windows | untested |

DVB cards are not yet available on Windows (planned via BDA); there is no Mac version yet.

## Included descriptions

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

## Settings per reception source

On the page of a source under **Settings → Reception**:

- **On/off** – disabled sources disappear from the header bar.
- **Device** – address of the receiver or the DVB card; plus the scan or loading the channel list
  from the device.
- **Channel list** – its own list with its own order (Manage channels).
- **Guide** – one of the XMLTV sources defined once under **Reception**, the guide from the signal
  (EIT, if the device delivers it) or both; XMLTV is never required.
- **Channel logos** – on or off per source.
- **Recordings** – limit for simultaneous recordings (e.g. by the number of tuners).

## Writing your own description

Your own descriptions are `.toml` files in the `adapter` folder of the configuration:

- Linux: `~/.config/tvjoe/adapter/`
- Windows: `%APPDATA%\tvjoe\adapter\`

They appear under “+” the next time the settings are opened. A file with the same `id` as an
included description replaces it. A file with errors is skipped; tvjoe only names the reason with
`TVJOE_TRACE=1` on the command line.

### Example: SAT>IP receiver for Astra 19.2° East

```toml
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

### Example: device with its own channel list (like the FRITZ!Box)

```toml
id = "my-cable"
name = "My cable SAT>IP"
driver = "satip"
systems = ["dvbc"]
tuners = 4
signal_epg = false
channel_list = ["http://{host}/dvb/m3u/tvhd.m3u"]
```

### Fields

| Field | Required | Meaning |
|---|---|---|
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

## Limits

- A description can only use existing drivers. A new transmission path (e.g. a device with its own
  protocol) needs code in the program.
- Your own frequency plans (other countries) cannot be added as files yet.
- On Linux, DVB cards need a kernel driver, often firmware too, and the user needs access to
  `/dev/dvb` (group `video`).
