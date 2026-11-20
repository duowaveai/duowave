# DuoWave onto a Yandex Station from this computer, in one line (Windows 10 and later, PowerShell):
#   irm https://github.com/duowaveai/duowave/releases/latest/download/install.ps1 | iex
# With the station's address given beforehand (`irm | iex` passes no arguments):
#   $env:DUOWAVE_STATION = '192.168.1.23'; irm https://github.com/duowaveai/duowave/releases/latest/download/install.ps1 | iex
# It downloads DuoWave for this computer (the gateway alone: the station's signed build is inside it), checks the file
# against the release's SHA256SUMS and runs `duowave station setup` (gateway/src/placement/setup.rs): the station is
# found in the home network, looked at over SSH, nothing changes before your "yes", then DuoWave goes onto the
# station. Nothing stays on this computer: the program lives in a temporary folder, removed at the end.
# The station needs root and SSH first: https://hackalice.me/
#
# This file is ASCII only, on purpose. Windows PowerShell 5.1's `irm` reads a script served without a charset as
# Latin-1, so Cyrillic written here arrived as mojibake (test 21.11: Latin letters with accents for the download line). The Russian
# messages are written as \uXXXX escapes and made into text when the script runs (function U).
$ErrorActionPreference = 'Stop'

$repo = if ($env:DUOWAVE_REPO) { $env:DUOWAVE_REPO } else { 'duowaveai/duowave' }
$version = if ($env:DUOWAVE_VERSION) { $env:DUOWAVE_VERSION } else { 'latest' }
$ru = (Get-Culture).TwoLetterISOLanguageName -eq 'ru'
# \uXXXX escapes into text: what keeps the Russian messages whole however the script was read.
function U([string]$text) { [regex]::Unescape($text) }
function Say($ruText, $enText) { if ($ru) { Write-Host $ruText } else { Write-Host $enText } }
function Die($ruText, $enText) { if ($ru) { throw $ruText } else { throw $enText } }

$base = if ($version -eq 'latest') { "https://github.com/$repo/releases/latest/download" } else { "https://github.com/$repo/releases/download/$version" }
# One build for Windows: x86_64 (Windows on ARM runs it too).
$name = 'duowave-windows-x86_64.tar.gz'
$dir = Join-Path ([System.IO.Path]::GetTempPath()) ("duowave-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $dir | Out-Null
$oldLanguage = $env:LANG
$oldEncoding = $null
try {
    Say (U '\u0421\u043a\u0430\u0447\u0438\u0432\u0430\u044e DuoWave \u0434\u043b\u044f \u044d\u0442\u043e\u0433\u043e \u043a\u043e\u043c\u043f\u044c\u044e\u0442\u0435\u0440\u0430\u2026') "Downloading DuoWave for this computer..."
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile (Join-Path $dir $name)
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile (Join-Path $dir 'SHA256SUMS')
    } catch {
        Die (U '\u0412 \u0432\u044b\u043f\u0443\u0441\u043a\u0435 \u043d\u0435\u0442 SHA256SUMS \u2014 \u043f\u0440\u043e\u0432\u0435\u0440\u0438\u0442\u044c \u0444\u0430\u0439\u043b \u043d\u0435\u0447\u0435\u043c, \u043d\u0435 \u0437\u0430\u043f\u0443\u0441\u043a\u0430\u044e.') "The release has no SHA256SUMS - nothing to check the file against, not running it."
    }
    $want = $null
    foreach ($line in Get-Content (Join-Path $dir 'SHA256SUMS')) {
        $parts = $line -split '\s+', 2
        if ($parts.Count -eq 2 -and ($parts[1] -eq $name -or $parts[1] -eq "*$name")) { $want = $parts[0].ToLower(); break }
    }
    if (-not $want) { Die ((U '\u0412 SHA256SUMS \u043d\u0435\u0442 {0} \u2014 \u043d\u0435 \u0437\u0430\u043f\u0443\u0441\u043a\u0430\u044e.') -f $name) "$name is not in SHA256SUMS - not running it." }
    $got = (Get-FileHash -Algorithm SHA256 (Join-Path $dir $name)).Hash.ToLower()
    if ($want -ne $got) { Die (U '\u041a\u043e\u043d\u0442\u0440\u043e\u043b\u044c\u043d\u0430\u044f \u0441\u0443\u043c\u043c\u0430 \u043d\u0435 \u0441\u043e\u0432\u043f\u0430\u043b\u0430: \u0444\u0430\u0439\u043b \u043f\u043e\u0432\u0440\u0435\u0436\u0434\u0451\u043d \u0438\u043b\u0438 \u043f\u043e\u0434\u043c\u0435\u043d\u0451\u043d. \u041f\u043e\u043f\u0440\u043e\u0431\u0443\u0439\u0442\u0435 \u0435\u0449\u0451 \u0440\u0430\u0437.') "The checksum does not match: the file is damaged or replaced. Try again." }

    tar -xzf (Join-Path $dir $name) -C $dir duowave.exe
    if ($LASTEXITCODE -ne 0) { Die (U '\u041d\u0435 \u0440\u0430\u0441\u043f\u0430\u043a\u043e\u0432\u0430\u043b\u043e\u0441\u044c.') "Could not unpack." }
    # The setup speaks the language of this Windows, in UTF-8: where PowerShell reads the program's output through a
    # pipe (inside `iex` it may), it decodes it with the console's encoding, the OEM code page unless told otherwise.
    $env:LANG = if ($ru) { 'ru_RU.UTF-8' } else { 'en_US.UTF-8' }
    try {
        $oldEncoding = [Console]::OutputEncoding
        [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
    } catch {
        $oldEncoding = $null
    }
    # The station's address: given to the script (`& ([scriptblock]::Create((irm <url>))) 192.168.1.23`) or in
    # DUOWAVE_STATION; else the setup finds the station itself, or asks.
    $station = @($args)
    if ($station.Count -eq 0 -and $env:DUOWAVE_STATION) { $station = @($env:DUOWAVE_STATION) }
    & (Join-Path $dir 'duowave.exe') station setup @station
} finally {
    $env:LANG = $oldLanguage
    if ($null -ne $oldEncoding) { try { [Console]::OutputEncoding = $oldEncoding } catch { } }
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
}
