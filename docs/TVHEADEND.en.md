# Tvheadend as a reception source

tvjoe can use a [Tvheadend](https://tvheadend.org/) server on your own network as a reception
source: the channels come from Tvheadend, playback runs through Tvheadend, and the guide comes
either from Tvheadend or from XMLTV. Where Tvheadend itself gets the channels from – DVB card or
stick, SAT>IP receiver, HDHomeRun, IPTV/M3U – does not matter to tvjoe.

Experimental. Tested with Tvheadend 4.3 (package from the project's repository, Debian 13) with
IPTV from an M3U list and with satellite through a SAT>IP receiver (EyeTV Netstream 4Sat, Astra
19.2° East).

## Preparing Tvheadend

1. **Channels:** set up network and tuners, scan, map services to channels (Tvheadend's wizard or
   Configuration → DVB Inputs). tvjoe takes over the *channels*, not the services.
2. **A user for tvjoe:** Configuration → Users → Access Entries: new entry with the user name,
   permission **Streaming** (default is enough) and access to the channels or channel tags you
   want. Under **Passwords** add a password for the same user. tvjoe does not need admin rights.
3. **Network:** tvjoe talks to Tvheadend over HTTP (port 9981). No exposure to the internet is
   needed; on a home network the IP address is enough.

By default Tvheadend only accepts HTTP digest authentication; tvjoe supports it (basic works too
if Tvheadend is set to it).

## Adding it in tvjoe

**Settings → Reception → “+” → Tvheadend**, then on the source's page:

1. **Address:** e.g. `192.0.2.5` or `http://tvheadend.example.org:9981` – without a scheme or port,
   `http://` and `9981` apply.
2. **User** and **password** of the Tvheadend user.
3. **Check:** signs in and shows the version (“Connected: Tvheadend 4.3-…”).
4. **Fetch channels:** takes over the enabled channels in Tvheadend's order. Fetching again keeps
   numbers, your own names and guide assignments; new channels are added, removed ones are
   marked “no longer found”.

The password is stored readable only by you in `~/.config/tvjoe/konten/tvheadend-<host>.toml`
(Windows: `%APPDATA%\tvjoe\konten\…`). The channel list holds only
`tvheadend://<host:port>/<channel>` per channel. When switching, tvjoe fetches a time-limited
**ticket** for the channel from Tvheadend and plays the address with that ticket – so the
password never appears in a stream address.

## Guide (EPG)

New channels get their guide **from Tvheadend** (`/api/epg/events/grid`). tvjoe fetches it at the
configured interval (“Fetch from Tvheadend every … hours”, one request to the server, no tuner
needed). Per channel you can choose in **Manage channels** → EPG between “From Tvheadend” and a
channel from the source's XMLTV sources; XMLTV sources remain usable in addition.

Where Tvheadend gets its guide from is up to Tvheadend:

- **From the signal (DVB, SAT>IP):** Tvheadend collects the guide from the stream (“OTA EPG”) –
  occupying a tuner for a while.
- **IPTV:** the stream usually carries no guide. Tvheadend can read XMLTV itself, e.g. with a small
  grabber script (example below). Then enable the module under Configuration → Channel/EPG → EPG
  Grabber Modules and map the XMLTV channels to Tvheadend's channels under **EPG Grabber
  Channels** (Tvheadend maps identical names partly by itself).

Example grabber (`/usr/bin/tv_grab_example`, executable; Tvheadend detects `tv_grab_*` at
startup):

```sh
#!/bin/sh
# XMLTV grabber for Tvheadend: downloads an XMLTV file (gzipped here) and prints it.
case "$1" in
  --description) echo "Example (XMLTV)"; exit 0 ;;
  --capabilities) echo "baseline"; exit 0 ;;
  --version) echo "1.0"; exit 0 ;;
esac
exec curl -fsSL --max-time 300 https://example.org/epg.xml.gz | gunzip -c
```

The address of the XMLTV file is an example; which source fits your channels and under which
terms it may be used is your decision.

## Encrypted channels (pay TV)

If Tvheadend cannot descramble a channel (no CI module, no card), it logs “No descrambler” but
still answers tvjoe with HTTP 200 and then sends nothing. tvjoe therefore reads briefly before
playing (at most 5 seconds) and shows “No data from Tvheadend – encrypted channel without
descrambling (pay TV) or no free tuner” instead of waiting. For free-to-air channels this costs
about one second when switching.

## Sharing tuners

Tvheadend occupies tuners not only for watching but also for scans and for collecting the guide
from the signal. If other devices use the same SAT>IP receiver directly (e.g. tvjoe with the
“SAT” source), give Tvheadend only some of the tuners in its tuner settings (disable the others
there).

## Troubleshooting

- **“Tvheadend: wrong user name or password, or no permission”:** check the user's access entry
  and password (both must exist for the same name).
- **“Tvheadend reports no channels”:** no services are mapped to channels in Tvheadend yet, or the
  user has no access to the channel tags.
- **Channel without guide:** check the EPG assignment under Manage channels in tvjoe; in
  Tvheadend, check whether the channel is mapped to an EPG grabber channel.
- For details: `tvjoe --diagnose` writes a complete log (`~/.cache/tvjoe/diagnose-…log`); access
  tokens and tickets are left out.
