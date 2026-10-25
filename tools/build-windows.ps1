<#
.SYNOPSIS
    Baut tvjoe für Windows: Programme, Paketordner mit allen DLLs und GTK-Daten, signierte MSI.

.DESCRIPTION
    Nach dem Vorbild von remotejoe (scripts/build-msi.ps1). Ablauf:
      1. Bau mit der Rust-GNU-Toolchain gegen MSYS2 ucrt64 (GTK 4, libadwaita, libmpv). Der PATH zu
         ucrt64 gilt nur in diesem Lauf; System- und Benutzer-PATH bleiben unberührt.
      2. Bereitstellungsordner target\windows\stage im üblichen GTK-Aufbau: bin\ (Programme und alle
         benötigten DLLs aus ucrt64), lib\gdk-pixbuf-2.0 (Bildlader, u. a. SVG für die Symbole),
         share\glib-2.0\schemas, share\icons (Adwaita, hicolor), share\locale\de (deutsche Texte von
         GTK und libadwaita), licenses\ (Lizenztexte der mitgelieferten MSYS2-Pakete).
      3. Signatur der eigenen Programme mit dem documentjoe-Zertifikat (Cert:\CurrentUser\My).
      4. MSI mit WiX 3.14 (packaging\windows\tvjoe.wxs), pro Benutzer ohne Administratorrechte;
         MSI signieren, SHA-256-Prüfsumme daneben.
    Jeder Fehler wird mit Ursache und nächstem Schritt gemeldet. Ergebnis: target\windows\.

.PARAMETER Msys
    MSYS2-Ordner (Vorgabe C:\msys64).

.PARAMETER WixDir
    Ordner mit candle.exe und light.exe (Vorgabe: Umgebungsvariable WIX bzw. deren bin, sonst
    C:\Program Files (x86)\WiX Toolset v3.14\bin).

.PARAMETER Thumbprint
    Fingerabdruck des Signaturzertifikats in Cert:\CurrentUser\My (mit privatem Schlüssel).

.PARAMETER TimestampUrl
    Zeitstempeldienst (RFC 3161).

.PARAMETER NoSign
    Nicht signieren (Probeläufe; dann sind auch nicht eingecheckte Änderungen erlaubt).

.PARAMETER NoMsi
    Nur Programme und Paketordner bauen, keine MSI.
#>
param(
    [string]$Msys = 'C:\msys64',
    [string]$WixDir = '',
    [string]$Thumbprint = 'C595450070432A501AADD72366C837AFB268740A',
    [string]$TimestampUrl = 'http://timestamp.digicert.com',
    [switch]$NoSign,
    [switch]$NoMsi
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Fehler([string]$Text, [string]$Hilfe = '') {
    Write-Host "FEHLER: $Text" -ForegroundColor Red
    if ($Hilfe) { Write-Host "  Abhilfe: $Hilfe" -ForegroundColor Yellow }
    exit 1
}

function Schritt([string]$Text) {
    Write-Host "== $Text" -ForegroundColor Cyan
}

function Ausfuehren([string]$Was, [scriptblock]$Block) {
    & $Block
    if ($LASTEXITCODE -ne 0) { Fehler "$Was ist fehlgeschlagen (Rückgabewert $LASTEXITCODE)." 'Meldungen oberhalb prüfen.' }
}

$Root = Split-Path -Parent $PSScriptRoot
$Out = Join-Path $Root 'target\windows'
$Stage = Join-Path $Out 'stage'
$Ucrt = Join-Path $Msys 'ucrt64'
$Bash = Join-Path $Msys 'usr\bin\bash.exe'
$Toolchain = 'stable-x86_64-pc-windows-gnu'

Schritt 'Version, Stand und Werkzeuge'
$versionLine = Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { Fehler 'Version in Cargo.toml nicht gefunden.' }
$Version = $versionLine.Matches[0].Groups[1].Value
if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fehler "Version „$Version“ passt nicht zu einer MSI (nur a.b.c)." }
$head = (git -C $Root rev-parse --short HEAD).Trim()
$geaendert = [bool](git -C $Root status --porcelain)
Write-Host "Version $Version, Stand $head$(if ($geaendert) { ' (mit nicht eingecheckten Änderungen)' })"
if (-not $NoSign -and $geaendert) {
    Fehler 'Nicht eingecheckte Änderungen; so wird nicht signiert.' 'Änderungen einchecken oder -NoSign für einen Probelauf.'
}

if (-not (Test-Path (Join-Path $Ucrt 'bin\libgtk-4-1.dll'))) {
    Fehler "GTK 4 fehlt unter $Ucrt." "In MSYS2: pacman -S --needed mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-gtk4 mingw-w64-ucrt-x86_64-libadwaita mingw-w64-ucrt-x86_64-mpv mingw-w64-ucrt-x86_64-librsvg"
}
if (-not (Test-Path $Bash)) { Fehler "MSYS2-Shell fehlt: $Bash" }
if (-not ((rustup toolchain list) -match [regex]::Escape($Toolchain))) {
    Fehler "Rust-Toolchain $Toolchain fehlt." "rustup toolchain install $Toolchain"
}

$signtool = $null
if (-not $NoSign) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending | Select-Object -First 1 -ExpandProperty FullName
    if (-not $signtool) { Fehler 'signtool.exe nicht gefunden.' 'Windows SDK installieren oder -NoSign.' }
    $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object Thumbprint -eq $Thumbprint
    if (-not $cert) { Fehler "Zertifikat $Thumbprint fehlt in Cert:\CurrentUser\My." 'Zertifikat mit privatem Schlüssel einspielen oder -Thumbprint angeben.' }
    if (-not $cert.HasPrivateKey) { Fehler "Zum Zertifikat $Thumbprint fehlt der private Schlüssel." }
}

if (-not $NoMsi) {
    if (-not $WixDir) {
        # Umgebungsvariable WIX, sonst die Ablage von remotejoe, sonst der Installationsordner von WiX.
        $kandidaten = @()
        if ($env:WIX) { $kandidaten += @((Join-Path $env:WIX 'bin'), $env:WIX) }
        $kandidaten += @('C:\Program Files (x86)\WiX Toolset v3.14\bin')
        $WixDir = $kandidaten | Where-Object { Test-Path (Join-Path $_ 'candle.exe') } | Select-Object -First 1
        if (-not $WixDir) { $WixDir = $kandidaten[-1] }
    }
    $candle = Join-Path $WixDir 'candle.exe'
    $light = Join-Path $WixDir 'light.exe'
    if (-not (Test-Path $candle) -or -not (Test-Path $light)) {
        Fehler "WiX nicht gefunden unter $WixDir." 'WiX 3.14 auspacken und -WixDir angeben, oder -NoMsi.'
    }
}

Schritt 'Programme bauen'
# Nur für diesen Lauf: ucrt64 vorn im PATH (gcc, windres, pkg-config, DLLs). Die MSYS2-Shell
# (ldd, pacman) braucht MSYSTEM=UCRT64, sonst setzt ihr Login-Profil einen PATH ohne /ucrt64/bin.
$env:PATH = "$Ucrt\bin;$env:PATH"
$env:MSYSTEM = 'UCRT64'
$env:PKG_CONFIG = Join-Path $Ucrt 'bin\pkg-config.exe'
Push-Location $Root
try {
    Ausfuehren 'cargo build' { cargo "+$Toolchain" build --release }
} finally {
    Pop-Location
}
$bauOrdner = Join-Path $Root 'target\release'

Schritt 'Bereitstellungsordner'
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
$bin = Join-Path $Stage 'bin'
New-Item -ItemType Directory -Force $bin | Out-Null
foreach ($exe in @('tvjoe.exe', 'tvjoe-gui.exe')) {
    $p = Join-Path $bauOrdner $exe
    if (-not (Test-Path $p)) { Fehler "$exe fehlt nach dem Bau." }
    Copy-Item $p $bin
}

# GTK-Daten im üblichen Aufbau neben bin\ (GTK sucht sie relativ zum Ordner seiner DLLs).
$pixbuf = 'lib\gdk-pixbuf-2.0\2.10.0'
$daten = @(
    $pixbuf,
    'share\glib-2.0\schemas\gschemas.compiled',
    'share\icons\Adwaita',
    'share\icons\hicolor\index.theme'
)
foreach ($d in $daten) {
    $quelle = Join-Path $Ucrt $d
    if (-not (Test-Path $quelle)) { Fehler "GTK-Daten fehlen: $quelle" 'MSYS2-Pakete wie oben installieren (adwaita-icon-theme kommt mit gtk4).' }
    $ziel = Join-Path $Stage $d
    New-Item -ItemType Directory -Force (Split-Path -Parent $ziel) | Out-Null
    Copy-Item $quelle $ziel -Recurse -Force
}
# Deutsche Texte von GTK und libadwaita.
$loc = 'share\locale\de\LC_MESSAGES'
New-Item -ItemType Directory -Force (Join-Path $Stage $loc) | Out-Null
foreach ($mo in @('gtk40.mo', 'gtk40-properties.mo', 'libadwaita.mo', 'glib20.mo')) {
    $q = Join-Path $Ucrt "$loc\$mo"
    if (Test-Path $q) { Copy-Item $q (Join-Path $Stage $loc) }
}

Schritt 'DLLs einsammeln'
# Abhängigkeiten der Programme und der Bildlader rekursiv mit ldd (MSYS2); nur DLLs aus ucrt64.
function Msys-Pfad([string]$Pfad) { '/' + $Pfad.Substring(0, 1).ToLower() + ($Pfad.Substring(2) -replace '\\', '/') }
$pruefen = New-Object System.Collections.Generic.Queue[string]
Get-ChildItem $bin -Filter '*.exe' | ForEach-Object { $pruefen.Enqueue($_.FullName) }
Get-ChildItem (Join-Path $Stage "$pixbuf\loaders") -Filter '*.dll' | ForEach-Object { $pruefen.Enqueue($_.FullName) }
$gesehen = @{}
while ($pruefen.Count -gt 0) {
    $datei = $pruefen.Dequeue()
    $zeilen = & $Bash -lc "ldd '$(Msys-Pfad $datei)'" 2>$null
    foreach ($z in $zeilen) {
        if ($z -match '=>\s+(/ucrt64/bin/[^\s]+\.dll)') {
            $name = Split-Path -Leaf $Matches[1]
            if ($gesehen.ContainsKey($name)) { continue }
            $gesehen[$name] = $true
            $q = Join-Path $Ucrt "bin\$name"
            Copy-Item $q $bin
            $pruefen.Enqueue($q)
        }
    }
}
if (-not $gesehen.ContainsKey('libgtk-4-1.dll') -or -not $gesehen.ContainsKey('libmpv-2.dll')) {
    Fehler 'ldd hat GTK oder libmpv nicht gefunden; das Paket wäre unvollständig.' 'Ausgabe von ldd für bin\tvjoe-gui.exe prüfen.'
}
Write-Host "$($gesehen.Count) DLLs aus ucrt64"

Schritt 'Lizenzen'
$lic = Join-Path $Stage 'licenses'
New-Item -ItemType Directory -Force $lic | Out-Null
foreach ($l in @('LICENSE', 'THIRD_PARTY.md', 'THIRD_PARTY_LICENSES.txt')) {
    $p = Join-Path $Root $l
    if (-not (Test-Path $p)) { Fehler "Lizenzdatei fehlt: $p" }
    Copy-Item $p $Stage
}
# Lizenztexte der Pakete, aus denen die mitgelieferten DLLs stammen (pacman -Qo).
$pakete = @{}
foreach ($name in $gesehen.Keys) {
    $o = & $Bash -lc "pacman -Qoq /ucrt64/bin/$name" 2>$null
    if ($o) { $pakete[$o.Trim()] = $true }
}
foreach ($pk in $pakete.Keys) {
    $q = Join-Path $Ucrt "share\licenses\$($pk -replace '^mingw-w64-ucrt-x86_64-', '')"
    if (Test-Path $q) { Copy-Item $q (Join-Path $lic ($pk -replace '^mingw-w64-ucrt-x86_64-', '')) -Recurse -Force }
}
Write-Host "Lizenztexte von $((Get-ChildItem $lic -Directory).Count) Paketen"

if (-not $NoSign) {
    Schritt 'Eigene Programme signieren'
    $eigene = Get-ChildItem $bin -Filter 'tvjoe*.exe'
    if ($eigene.Count -ne 2) { Fehler "Erwartet tvjoe.exe und tvjoe-gui.exe, gefunden: $($eigene.Name -join ', ')." }
    Ausfuehren 'signtool sign (Programme)' { & $signtool sign /sha1 $Thumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 /d 'tvjoe' $eigene.FullName }
    foreach ($e in $eigene) {
        Ausfuehren "Signatur prüfen ($($e.Name))" { & $signtool verify /pa /q $e.FullName }
    }
}

$groesse = (Get-ChildItem $Stage -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("Paketordner: {0} ({1} Dateien, {2:N0} MB)" -f $Stage, (Get-ChildItem $Stage -Recurse -File).Count, ($groesse / 1MB))
if ($NoMsi) { Write-Host 'Fertig (ohne MSI).'; exit 0 }

Schritt 'Dateiliste für WiX'
# Je Ordner eine Komponente mit Schlüssel in HKCU (pro Benutzer vorgeschrieben) und RemoveFolder;
# feste GUIDs aus dem relativen Pfad, damit Updates dieselben Komponenten erkennen.
$md5 = [Security.Cryptography.MD5]::Create()
function Guid-Aus([string]$Text) {
    $b = $md5.ComputeHash([Text.Encoding]::UTF8.GetBytes("tvjoe-msi:$Text"))
    $b[6] = ($b[6] -band 0x0f) -bor 0x30; $b[8] = ($b[8] -band 0x3f) -bor 0x80
    return ([guid]::new([byte[]]$b)).ToString().ToUpper()
}
function Id-Aus([string]$Prefix, [string]$Text) {
    $h = -join ($md5.ComputeHash([Text.Encoding]::UTF8.GetBytes($Text))[0..7] | ForEach-Object { $_.ToString('x2') })
    return "${Prefix}_$h"
}
function Xml([string]$s) { [Security.SecurityElement]::Escape($s) }
$sb = New-Object Text.StringBuilder
[void]$sb.AppendLine('<?xml version="1.0" encoding="utf-8"?>')
[void]$sb.AppendLine('<!-- Erzeugt von tools/build-windows.ps1, nicht von Hand ändern. -->')
[void]$sb.AppendLine('<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi"><Fragment>')
$comps = New-Object Collections.Generic.List[string]
function Ordner([string]$Pfad, [string]$Rel) {
    $files = @(Get-ChildItem $Pfad -File | Sort-Object Name)
    $compId = Id-Aus 'c' "dir:$Rel"
    $comps.Add($compId)
    [void]$sb.AppendLine("<Component Id=`"$compId`" Guid=`"$(Guid-Aus "dir:$Rel")`" Win64=`"yes`">")
    [void]$sb.AppendLine("<RegistryValue Root=`"HKCU`" Key=`"Software\tvjoe\Installer\Ordner`" Name=`"$(Xml ($(if ($Rel) { $Rel } else { '.' })))`" Type=`"integer`" Value=`"1`" KeyPath=`"yes`" />")
    [void]$sb.AppendLine("<RemoveFolder Id=`"$(Id-Aus 'r' "dir:$Rel")`" On=`"uninstall`" />")
    foreach ($f in $files) {
        $relFile = if ($Rel) { "$Rel\$($f.Name)" } else { $f.Name }
        [void]$sb.AppendLine("<File Id=`"$(Id-Aus 'f' "file:$relFile")`" Name=`"$(Xml $f.Name)`" Source=`"$(Xml $f.FullName)`" KeyPath=`"no`" />")
    }
    [void]$sb.AppendLine('</Component>')
    foreach ($d in (Get-ChildItem $Pfad -Directory | Sort-Object Name)) {
        $relDir = if ($Rel) { "$Rel\$($d.Name)" } else { $d.Name }
        [void]$sb.AppendLine("<Directory Id=`"$(Id-Aus 'd' "dir:$relDir")`" Name=`"$(Xml $d.Name)`">")
        Ordner $d.FullName $relDir
        [void]$sb.AppendLine('</Directory>')
    }
}
[void]$sb.AppendLine('<DirectoryRef Id="INSTALLDIR">')
Ordner $Stage ''
[void]$sb.AppendLine('</DirectoryRef>')
[void]$sb.AppendLine('<ComponentGroup Id="AppFiles">')
foreach ($c in $comps) { [void]$sb.AppendLine("<ComponentRef Id=`"$c`" />") }
[void]$sb.AppendLine('</ComponentGroup>')
[void]$sb.AppendLine('</Fragment></Wix>')
$filesWxs = Join-Path $Out 'files.wxs'
[IO.File]::WriteAllText($filesWxs, $sb.ToString(), (New-Object Text.UTF8Encoding $false))
Write-Host "$($comps.Count) Ordner"

Schritt 'MSI bauen (candle, light)'
$wxs = Join-Path $Root 'packaging\windows\tvjoe.wxs'
$ico = Join-Path $Root 'packaging\windows\tvjoe.ico'
$obj = Join-Path $Out 'obj'
New-Item -ItemType Directory -Force $obj | Out-Null
$msiName = "tvjoe-$Version-windows-x86_64.msi"
$msi = Join-Path $Out $msiName
Ausfuehren 'candle' { & $candle -nologo -arch x64 -ext WixUtilExtension "-dVersion=$Version" "-dIconFile=$ico" -out "$obj\" $wxs $filesWxs }
# ICE91: Dateien im Benutzerprofil bei einer Installation pro Benutzer (gewollt).
# ICE61: Dieselbe Version ersetzt sich selbst (AllowSameVersionUpgrades; gewollt).
Ausfuehren 'light' { & $light -nologo -spdb -ext WixUtilExtension -sice:ICE91 -sice:ICE61 -out $msi "$obj\tvjoe.wixobj" "$obj\files.wixobj" }

if (-not $NoSign) {
    Schritt 'MSI signieren'
    Ausfuehren 'signtool sign (MSI)' { & $signtool sign /sha1 $Thumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 /d "tvjoe $Version" $msi }
}

Schritt 'Prüfsumme'
$hash = (Get-FileHash $msi -Algorithm SHA256).Hash.ToLower()
[IO.File]::WriteAllText("$msi.sha256", "$hash  $msiName`n", (New-Object Text.UTF8Encoding $false))
Write-Host ("Fertig: {0} ({1:N1} MB)" -f $msi, ((Get-Item $msi).Length / 1MB))
Write-Host "SHA-256: $hash"
