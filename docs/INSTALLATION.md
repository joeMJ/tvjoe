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

**Erst lesen, dann ausführen** (empfohlen): Der Einzeiler oben führt das Skript aus, ohne dass du
es vorher siehst. Wer das nicht will, lädt es zuerst, liest es und startet es dann:

```bash
curl -fsSL -o install-tvjoe.sh https://raw.githubusercontent.com/joeMJ/tvjoe/main/install.sh
less install-tvjoe.sh
bash install-tvjoe.sh
```

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

**Voraussetzungen:** Windows 10 oder 11 (64 Bit), keine Administratorrechte nötig. Für das Videobild
braucht tvjoe OpenGL vom Grafiktreiber: direkt am Rechner starten. In einer Remotedesktop-Sitzung
oder in einer VM ohne 3D-Beschleunigung gibt es kein taugliches OpenGL; tvjoe meldet das dann.

**Installieren:** die Datei `tvjoe-<Version>-windows-x86_64.msi` von der
[Release-Seite](https://github.com/joeMJ/tvjoe/releases/latest) laden, die Prüfsumme mit der
Datei `….msi.sha256` daneben vergleichen und die MSI doppelklicken. Sie installiert nur für das
eigene Benutzerkonto nach `%LOCALAPPDATA%\Programs\tvjoe`, legt eine Verknüpfung im Startmenü an und
erscheint unter „Apps und Features“. Alle benötigten Bibliotheken (GTK, libadwaita, libmpv) sind
enthalten.

Die MSI und die Programme sind mit demselben selbst ausgestellten Zertifikat signiert wie remotejoe.
Wer es schon für remotejoe eingetragen hat, muss nichts weiter tun.

**SAT>IP:** Die Windows-Firewall verwirft den Datenstrom des SAT>IP-Geräts zunächst stillschweigend.
Unter **Einstellungen → Empfang → EyeTV Netstream 4Sat → Prüfen** bietet tvjoe dann **Freigabe eintragen …** an, siehe
[SAT-IP.md](SAT-IP.md#firewall).

**Aktualisieren:** Die Updateprüfung (Einstellungen → Updates) meldet eine neue Version, lädt auf
Wunsch die MSI, prüft sie gegen die `.sha256`-Datei und startet den Installer; tvjoe beendet sich
dafür. Das geht ab 0.1.2; ältere Fassungen öffnen nur die Release-Seite bzw. melden „Distribution
unbekannt“ – dann die neue MSI einmal von Hand über die alte installieren.

**Deinstallieren:** unter „Apps und Features“ (bzw. „Installierte Apps“) bei **tvjoe**. Einstellungen
und Senderlisten (`%APPDATA%\tvjoe`) sowie der Zwischenspeicher (`%LOCALAPPDATA%\tvjoe`) bleiben
erhalten.

> [!WARNING]
> **Zertifikat des Herausgebers: freiwillig und mit Risiko.**
> Ohne eingetragenes Zertifikat meldet Windows bei der Installation einen „unbekannten Herausgeber“ –
> tvjoe funktioniert trotzdem genauso, man bestätigt die Warnung nur von Hand. Das Eintragen legt ein
> selbst ausgestelltes Zertifikat für den **ganzen Rechner** unter den vertrauenswürdigen
> Stammzertifizierungsstellen ab. Nur eintragen, wenn du dem Herausgeber vertraust; was das bedeutet,
> steht unter [Zertifikat des Herausgebers](#zertifikat-des-herausgebers). Im Zweifel nicht eintragen und
> stattdessen die Prüfsumme der MSI vergleichen.

> [!NOTE]
> **Warnung „Der Computer wurde durch Windows geschützt“:** Microsoft Defender SmartScreen zeigt diese
> Meldung bei Dateien, die noch selten heruntergeladen wurden – auch wenn sie signiert sind. So geht es weiter:
>
> 1. Auf **Weitere Informationen** klicken.
> 2. Prüfen, dass bei **Herausgeber** steht: `documentjoe, documentjoe Code Signing (privat)`.
> 3. Nur dann **Trotzdem ausführen** wählen.
>
> Steht dort „Unbekannter Herausgeber“, ist entweder das [Zertifikat](#zertifikat-des-herausgebers)
> nicht eingetragen oder die Datei wurde verändert. Dann vor dem Ausführen die Prüfsumme der Datei mit
> der Angabe auf der Release-Seite vergleichen (PowerShell im Download-Ordner):
>
> ```powershell
> Get-FileHash .\<Dateiname> -Algorithm SHA256
> ```

### Zertifikat des Herausgebers

Die Windows-Pakete sind mit einem **selbst ausgestellten** Zertifikat signiert. Damit Windows bei der
Installation keinen „unbekannten Herausgeber“ meldet, kann man den öffentlichen Teil dieses Zertifikats
einmalig als vertrauenswürdig eintragen.

- Datei: [`docs/zertifikat/documentjoe-codesigning.cer`](zertifikat/documentjoe-codesigning.cer)
- Name: `CN=documentjoe Code Signing (privat), O=documentjoe`, gültig bis 07.10.2041
- Verwendung: **nur Code-Signatur** (Schlüsselverwendung „Digitale Signatur“, erweiterte Verwendung „Codesignatur“).
  Es ist keine Zertifizierungsstelle und kann keine anderen Zertifikate ausstellen; für Webseiten (TLS) oder
  E-Mail taugt es nicht.
- Fingerabdruck (SHA-1): `C595450070432A501AADD72366C837AFB268740A`
- Dasselbe Zertifikat signiert auch remotejoe.

**Was das Eintragen bedeutet:** Das Skript legt das Zertifikat in die Speicher
**„Vertrauenswürdige Stammzertifizierungsstellen“** und **„Vertrauenswürdige Herausgeber“** des
Rechners, also für alle Benutzer. Ab dann hält Windows **jedes** Programm für vertrauenswürdig, das mit
dem zugehörigen privaten Schlüssel signiert ist – nicht nur tvjoe. Solange nur der Herausgeber diesen
Schlüssel hat, ist das gewollt; käme er abhanden, könnte damit auch Schadsoftware als „vertrauenswürdig“
erscheinen. Die Einschränkung auf Code-Signatur (siehe oben) verhindert Missbrauch für Webseiten oder
E-Mail, nicht für Programme. Deshalb:

- Nur eintragen, wenn du dem Herausgeber vertraust und tvjoe regelmäßig nutzt.
- Ohne Eintrag bleibt alles benutzbar; die Echtheit der MSI prüfst du dann über die Prüfsumme auf der
  Release-Seite (`Get-FileHash`, siehe oben).
- Wird das Zertifikat nicht mehr gebraucht, mit dem Befehl unter **Wieder entfernen** (unten) austragen.

**Eintragen** – PowerShell **als Administrator** öffnen, den ganzen Block kopieren und einfügen
(Windows 10 und 11, auch im eingeschränkten Sprachmodus). Das Skript lädt die Datei, prüft Prüfsumme
(SHA-256) und Fingerabdruck und trägt das Zertifikat nur dann ein, wenn beides stimmt. Ist es schon
eingetragen (z. B. für remotejoe), tut es nichts.

```powershell
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference    = 'SilentlyContinue'
    $url    = 'https://raw.githubusercontent.com/joeMJ/tvjoe/main/docs/zertifikat/documentjoe-codesigning.cer'
    $thumb  = 'C595450070432A501AADD72366C837AFB268740A'
    $sha256 = '6F672EFA23A39A4C26693BA4F979A4368E657400BCB9EA8E05214E29C80E5DAE'
    $stores = [ordered]@{ 'Root' = 'Vertrauenswürdige Stammzertifizierungsstellen'; 'TrustedPublisher' = 'Vertrauenswürdige Herausgeber' }
    $file   = Join-Path $env:TEMP 'documentjoe-codesigning.cer'
    function Step($t) { Write-Host "  ..   $t" -ForegroundColor Gray }
    function Ok($t)   { Write-Host "  OK   $t" -ForegroundColor Green }
    function Fail($t) { Write-Host "  FEHLER  $t" -ForegroundColor Red }

    Write-Host ''
    Write-Host '  tvjoe - Zertifikat des Herausgebers eintragen' -ForegroundColor Cyan
    Write-Host '  =============================================' -ForegroundColor Cyan
    Write-Host ''

    if (-not (whoami /groups | Select-String -SimpleMatch 'S-1-16-12288' -Quiet)) {
        Fail 'Diese PowerShell läuft nicht als Administrator.'
        Write-Host '         Bitte PowerShell mit "Als Administrator ausführen" öffnen und erneut einfügen.' -ForegroundColor Yellow
        Write-Host ''
        return
    }

    $missing = @($stores.Keys | Where-Object { -not (Test-Path "Cert:\LocalMachine\$_\$thumb") })
    if ($missing.Count -eq 0) {
        Ok 'Das Zertifikat ist bereits eingetragen. Es ist nichts zu tun.'
        Write-Host ''
        return
    }

    try {
        Step 'Zertifikat von GitHub laden'
        try {
            Invoke-WebRequest -Uri $url -OutFile $file -UseBasicParsing
        } catch {
            & curl.exe -fsSL -o $file $url
            if ($LASTEXITCODE -ne 0) { throw 'Das Zertifikat konnte nicht geladen werden. Bitte die Internetverbindung prüfen.' }
        }
        Ok 'Geladen'

        Step 'Echtheit prüfen'
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash -ne $sha256) {
            throw 'Die Prüfsumme stimmt nicht. Das Zertifikat wird NICHT eingetragen.'
        }
        $cert = Get-PfxCertificate -FilePath $file
        if (-not $cert -or $cert.Thumbprint -ne $thumb) {
            throw 'Der Fingerabdruck stimmt nicht. Das Zertifikat wird NICHT eingetragen.'
        }
        Ok "Echt: $($cert.Subject)"
        Write-Host "         Fingerabdruck $thumb, gültig bis $($cert.NotAfter.ToString('dd.MM.yyyy'))" -ForegroundColor Gray

        foreach ($s in $missing) {
            Step "Eintragen in: $($stores[$s])"
            Import-Certificate -FilePath $file -CertStoreLocation "Cert:\LocalMachine\$s" | Out-Null
            Ok 'Eingetragen'
        }
        Write-Host ''
        Write-Host '  Fertig. Programme und Pakete von tvjoe (und remotejoe) werden jetzt als vertrauenswürdig erkannt.' -ForegroundColor Green
    } catch {
        Fail $_.Exception.Message
    } finally {
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    }
    Write-Host ''
}
```

Das Eintragen ist **freiwillig**. Weil das Zertifikat selbst ausgestellt ist, braucht Windows es an zwei Stellen:
unter „Vertrauenswürdige Stammzertifizierungsstellen“, damit die Signatur überhaupt als gültig gilt, und unter
„Vertrauenswürdige Herausgeber“, damit Windows den Herausgeber kennt. Danach vertraut Windows allem, was mit diesem
Schlüssel signiert ist – nur als Programmsignatur, nicht für Webseiten oder E-Mail. Das verbleibende Risiko: Wer den
privaten Schlüssel hätte, könnte Programme signieren, denen dieser Rechner dann vertraut. Wer das nicht möchte, lässt
es weg und bestätigt die Warnung bei der Installation von Hand. Die Warnung von SmartScreen zu selten
heruntergeladenen Dateien kann trotzdem erscheinen; sie hängt nicht am Zertifikat.

**Wieder entfernen** – ebenfalls in einer PowerShell als Administrator einfügen. Das entfernt das Zertifikat
für alle Programme, die damit signiert sind, also auch für remotejoe:

```powershell
& {
    $thumb  = 'C595450070432A501AADD72366C837AFB268740A'
    $stores = [ordered]@{ 'Root' = 'Vertrauenswürdige Stammzertifizierungsstellen'; 'TrustedPublisher' = 'Vertrauenswürdige Herausgeber' }

    Write-Host ''
    Write-Host '  tvjoe - Zertifikat des Herausgebers entfernen' -ForegroundColor Cyan
    Write-Host '  =============================================' -ForegroundColor Cyan
    Write-Host ''

    if (-not (whoami /groups | Select-String -SimpleMatch 'S-1-16-12288' -Quiet)) {
        Write-Host '  FEHLER  Diese PowerShell läuft nicht als Administrator.' -ForegroundColor Red
        Write-Host '         Bitte PowerShell mit "Als Administrator ausführen" öffnen und erneut einfügen.' -ForegroundColor Yellow
        Write-Host ''
        return
    }

    foreach ($s in $stores.Keys) {
        $path = "Cert:\LocalMachine\$s\$thumb"
        if (Test-Path $path) {
            try {
                Remove-Item $path -ErrorAction Stop
                Write-Host "  OK   Entfernt aus: $($stores[$s])" -ForegroundColor Green
            } catch {
                Write-Host "  FEHLER  $($stores[$s]): $($_.Exception.Message)" -ForegroundColor Red
            }
        } else {
            Write-Host "  --   Nicht vorhanden in: $($stores[$s])" -ForegroundColor Gray
        }
    }
    Write-Host ''
}
```
