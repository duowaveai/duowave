# DuoWave onto a Yandex Station from this computer, in one line (Windows 10 and later, PowerShell):
#   irm https://github.com/duowaveai/duowave/releases/latest/download/install.ps1 | iex
# It downloads DuoWave for this computer (the gateway alone: the station's signed build is inside it), checks the file
# against the release's SHA256SUMS and runs `duowave station setup` (gateway/src/placement/setup.rs): the station is
# found in the home network, looked at over SSH, nothing changes before your «да», then DuoWave goes onto the
# station. Nothing stays on this computer: the program lives in a temporary folder, removed at the end.
# The station needs root and SSH first: https://hackalice.me/
$ErrorActionPreference = 'Stop'

$repo = if ($env:DUOWAVE_REPO) { $env:DUOWAVE_REPO } else { 'duowaveai/duowave' }
$version = if ($env:DUOWAVE_VERSION) { $env:DUOWAVE_VERSION } else { 'latest' }
$ru = (Get-Culture).TwoLetterISOLanguageName -eq 'ru'
function Say($ruText, $enText) { if ($ru) { Write-Host $ruText } else { Write-Host $enText } }
function Die($ruText, $enText) { if ($ru) { throw $ruText } else { throw $enText } }

$base = if ($version -eq 'latest') { "https://github.com/$repo/releases/latest/download" } else { "https://github.com/$repo/releases/download/$version" }
# One build for Windows: x86_64 (Windows on ARM runs it too).
$name = 'duowave-windows-x86_64.tar.gz'
$dir = Join-Path ([System.IO.Path]::GetTempPath()) ("duowave-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $dir | Out-Null
$oldLanguage = $env:LANG
try {
    Say "Скачиваю DuoWave для этого компьютера…" "Downloading DuoWave for this computer…"
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile (Join-Path $dir $name)
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile (Join-Path $dir 'SHA256SUMS')
    } catch {
        Die "В выпуске нет SHA256SUMS — проверить файл нечем, не запускаю." "The release has no SHA256SUMS — nothing to check the file against, not running it."
    }
    $want = $null
    foreach ($line in Get-Content (Join-Path $dir 'SHA256SUMS')) {
        $parts = $line -split '\s+', 2
        if ($parts.Count -eq 2 -and ($parts[1] -eq $name -or $parts[1] -eq "*$name")) { $want = $parts[0].ToLower(); break }
    }
    if (-not $want) { Die "В SHA256SUMS нет $name — не запускаю." "$name is not in SHA256SUMS — not running it." }
    $got = (Get-FileHash -Algorithm SHA256 (Join-Path $dir $name)).Hash.ToLower()
    if ($want -ne $got) { Die "Контрольная сумма не совпала: файл повреждён или подменён. Попробуйте ещё раз." "The checksum does not match: the file is damaged or replaced. Try again." }

    tar -xzf (Join-Path $dir $name) -C $dir duowave.exe
    if ($LASTEXITCODE -ne 0) { Die "Не распаковалось." "Could not unpack." }
    # The setup speaks the language of this Windows.
    $env:LANG = if ($ru) { 'ru_RU.UTF-8' } else { 'en_US.UTF-8' }
    & (Join-Path $dir 'duowave.exe') station setup @args
} finally {
    $env:LANG = $oldLanguage
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
}
