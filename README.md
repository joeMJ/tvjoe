<p align="center">
  <img src="data/io.github.joeMJ.tvjoe.png" alt="tvjoe" width="160">
</p>

# tvjoe

[Deutsch](README.de.md) · **English**

> **Lightweight streaming client for live TV on Linux and Windows.**
> TV from your own M3U lists, satellite through a SAT>IP receiver on your home network, the
> Mediathek of the German public broadcasters and your own Jellyfin server – in one window.

<p align="center">
  <a href="https://www.johnlose.de/unterstutzung-fur-john-joe-lose/"><img src="docs/knoepfe/danke.svg" alt="Say thanks to John" width="300"></a>
</p>
<p align="center">
  <a href="docs/INSTALLATION.md#installation-windows"><img src="docs/knoepfe/windows-en.svg" alt="Install on Windows" width="260"></a>
  &nbsp;
  <a href="docs/INSTALLATION.md#installation-ubuntu"><img src="docs/knoepfe/ubuntu-en.svg" alt="Install on Ubuntu" width="260"></a>
</p>

tvjoe does not ship any channel lists; channels come from your own lists or from your own SAT>IP
receiver.

- **Fast and frugal:** Rust, GTK4/libadwaita and libmpv with hardware decoding. Live TV in 1080p50
  uses 6–9 % of one CPU core (measured).
- **Four sources, one switch:** TV | SAT | Mediathek | Jellyfin in the header bar. Guide,
  Mediathek and channel management open as a layer over the picture; the sound keeps playing.
- **TV as you know it:** number pad, channel list with the current show, banner with Now/Next,
  guide grid from XMLTV.

The user interface is available in German and English (Settings → Controls → Language).

## Documentation

The detailed documentation is in German:

- [`docs/INSTALLATION.md`](docs/INSTALLATION.md) – installation on Ubuntu and Windows
- [`docs/ERSTE-SCHRITTE.md`](docs/ERSTE-SCHRITTE.md) – first steps: import a channel list, add a guide, sort, back up
- [`docs/SAT-IP.md`](docs/SAT-IP.md) – set up a SAT>IP receiver, channel scan, firewall, on the road via WireGuard
- [`docs/TVHEADEND.en.md`](docs/TVHEADEND.en.md) – Tvheadend as a reception source: user, channels, guide, pay TV, sharing tuners
- [`docs/ADAPTER.en.md`](docs/ADAPTER.en.md) – adapters for reception sources and libraries: classes, drivers, included adapters, writing and contributing your own
- [`docs/MEDIATHEK.md`](docs/MEDIATHEK.md) – Mediathek of the public broadcasters and Jellyfin
- [`docs/DATENSCHUTZ.md`](docs/DATENSCHUTZ.md) – privacy: every network access
- [`docs/BAUEN.md`](docs/BAUEN.md) – building it yourself, command line
- [`THIRD_PARTY.md`](THIRD_PARTY.md) – third-party libraries and their licences

> [!WARNING]
> **Private hobby project – unmaintained.**
> This repository is meant for my own use and is only public for convenience.
>
> * **No support promised:** Pull requests are not handled, feature requests are not promised. Bugs and requests can be reported (see [Reporting bugs and requests](#reporting-bugs-and-requests)) – but it may take a while until they are dealt with.
> * **No warranty, liability:** The program is provided free of charge as a private project “as is”. There is no claim to availability, freedom from errors, updates or support. I am liable for damage from its use only in cases of intent, gross negligence and injury to life, body or health; otherwise only according to the statutory provisions.
> * **Own environment:** Developed and tested only on my own computers (Ubuntu 24.04 and 26.04, Windows 10). The Windows version is new. It may fail on other systems.
> * **Network:** The program runs with the rights of your user account and connects to the sources you enter. Which other accesses there are is listed in [`docs/DATENSCHUTZ.md`](docs/DATENSCHUTZ.md). **Read the code before you install it.**
> * **No updates guaranteed:** There may be changes, breakage or deletion of the repository at any time without notice.

<p align="center">
  <img src="docs/screenshots/senderliste.webp" alt="Main window: channel list with the current show over the picture" width="760">
  <br>
  <em>Channel list over the picture (L shows and hides it)</em>
  <br><br>
  <img src="docs/screenshots/info.webp" alt="Full-screen info with show, description and reception" width="760">
  <br>
  <em>Full-screen info (key I), here a SAT channel with reception display</em>
  <br><br>
  <img src="docs/screenshots/programm.webp" alt="Guide grid with current shows" width="760">
  <br>
  <em>Guide grid (G)</em>
  <br><br>
  <img src="docs/screenshots/mediathek.webp" alt="Mediathek of the public broadcasters: series with preview images" width="760">
  <br>
  <em>Mediathek of the public broadcasters (film list from MediathekView)</em>
</p>

---

## Platforms

| Platform | Status | Note |
| :--- | :--- | :--- |
| **Linux (Ubuntu 24.04 / 26.04)** | Active | One package per Ubuntu version, installed by script ([Installation](docs/INSTALLATION.md#installation-ubuntu)) |
| **Windows 10/11** | New | Per-user MSI without administrator rights ([Installation](docs/INSTALLATION.md#installation-windows)); needs OpenGL from the graphics driver |

## Installation

- **Ubuntu 26.04 / 24.04** – [Installation (Ubuntu)](docs/INSTALLATION.md#installation-ubuntu):

  ```bash
  curl -fsSL https://raw.githubusercontent.com/joeMJ/tvjoe/main/install.sh | bash
  ```

- **Windows 10/11:** MSI from the [release page](https://github.com/joeMJ/tvjoe/releases/latest) –
  [Installation (Windows)](docs/INSTALLATION.md#installation-windows).

> [!IMPORTANT]
> **Windows: add the publisher's certificate before the first installation.** The MSI is signed with a
> self-issued certificate; without the certificate Windows reports an “unknown publisher”.
> Command, fingerprint and explanation: [Publisher certificate](docs/INSTALLATION.md#zertifikat-des-herausgebers) (German).

---

## What it can do

- **Channel list** with numbers, logos and the current show; banner when switching (number, name,
  quality, Now/Next, reception for SAT); full-screen info (I).
- **Switching** by number pad, arrow keys or click; quality levels SD/720p/1080p/UHD with
  fallback on disturbances.
- **M3U import** from file or URL with preview, check and reachability test; partial import.
- **Channel management:** rename, numbers, order (drag with auto-scroll at the edge, to the top, to
  the end), active/favourite, export as M3U8. Inactive channels do not count for the numbers; new
  channels from an import or scan are added inactive at the end and do not disturb the order.
- **SAT source** through a SAT>IP receiver (e.g. EyeTV Netstream 4Sat, Astra 19.2° East): check of
  device and reception, scan into a separate SAT channel list, reception display, guide from the
  signal (EIT) and/or from XMLTV – see [`docs/SAT-IP.md`](docs/SAT-IP.md).
- **Programme guide** from XMLTV (also `.xml.gz`), guide grid.
- **Mediathek** of the public broadcasters: series with seasons, films, search; versions merged;
  continue watching, next episode automatically.
- **Jellyfin** as a second Mediathek: sign-in via Quick Connect (no password in tvjoe), watched
  state synchronised with Jellyfin, bandwidth limit with transcoding on the server.
- **Plex** (experimental) the same way: sign-in with a code at plex.tv/link, several servers possible.
- **Emby** (experimental) the same way: sign-in with user name and password or an API key.
- **Deinterlacing** against combing on 576i/1080i (can be switched off).
- **Logos** for internet and SAT channels, switchable separately.
- **Backup:** export settings and both channel lists to one file and import them again (without the
  Jellyfin sign-in).
- **Update check** via the releases of this repository (off by default).

<p align="center">
  <img src="docs/screenshots/verwaltung.webp" alt="Channel management with numbers, logos, groups, qualities and guide assignment" width="760">
  <br>
  <em>Channel management (Ctrl+E)</em>
</p>

## Keys

| Key | Action |
|---|---|
| Digits, Enter | Choose a channel (pause adjustable) |
| ← / →, Page Up / Down | Change channel |
| ↑ / ↓, + / −, mouse wheel | Volume |
| Q | Quality one level higher |
| F, double-click / Esc | Fullscreen / leave fullscreen |
| M | Mute / unmute |
| L | Channel list on/off |
| I, Esc | Full-screen info on / off |
| A | Change audio track |
| U | Change subtitles (track 1 … last, then off) |
| Z | Fill picture / fit |
| Space | Pause live TV / continue time-shifted |
| Page Up / Down (time-shifted) | 1 minute back / forward |
| End | Back to live |
| R | Record the current show / stop recording |
| G | Guide grid |
| D | Storage (downloads and recordings) |
| Ctrl+E | Manage channels |
| Ctrl+, | Settings |
| Ctrl+Q | Quit |

When playing from the Mediathek or from Jellyfin:

| Key | Action |
|---|---|
| Space | Pause / continue |
| ← / → | 10 seconds back / forward |
| Page Up / Down | 1 minute back / forward |
| Digits, channel list | Back to TV |

## Privacy

tvjoe sends no usage data and has no telemetry. It connects to the sources you enter; every
network access in detail: [`docs/DATENSCHUTZ.md`](docs/DATENSCHUTZ.md) (German).

## Mediathek and MediathekView

The Mediathek uses the film list of [MediathekView](https://mediathekview.de/), a volunteer
project that indexes the Mediatheken of the German public broadcasters. tvjoe downloads the list
at most once a day; the videos come directly from the broadcasters.
MediathekView funds its servers through donations – if you use the Mediathek in tvjoe, you can
support the project here: [mediathekview.de/spenden](https://mediathekview.de/spenden/).

## Reporting bugs and requests

The easiest way is **Settings → Updates → “Report a bug / request a feature”**: it opens the
[issues on GitHub](https://github.com/joeMJ/tvjoe/issues) in the browser (GitHub account required).
It may take a while until an issue is dealt with.

Helpful are:

- Version (**Settings → Updates → About tvjoe**) and system (e.g. Ubuntu 26.04)
- Source (TV, SAT, Mediathek, Jellyfin) and what exactly happens
- Steps that reproduce the bug, and the exact wording of a message

**Never include:** the access key for Jellyfin (`~/.config/tvjoe/jellyfin.toml`) and channel lists
with credentials in their addresses. Please make internal addresses and names in screenshots
unrecognisable.

## Licence

Apache-2.0, see [LICENSE](LICENSE). Third-party libraries and their licences:
[THIRD_PARTY.md](THIRD_PARTY.md).

According to their copyright file, the Ubuntu packages of libmpv are under GPL-3+ (linked against
libsmbclient). These terms apply additionally to the distributed tvjoe binary packages;
Apache-2.0 is compatible with GPL-3.
