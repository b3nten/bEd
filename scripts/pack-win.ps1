# Portable bEd package translated from pinned scripts/build-win-ci.bat.
param([string]$Binary = "target/release/bed.exe", [string]$Destination = "target/dist")
$ErrorActionPreference = "Stop"
$repository = Split-Path -Parent $PSScriptRoot
Set-Location $repository
if (!(Test-Path -LiteralPath $Binary -PathType Leaf)) { throw "Build with cargo build --locked --release --bin bed first." }
$versionLine = Get-Content Cargo.toml | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
$null = $versionLine -match '^version = "([^"]+)"'
$version = $Matches[1]
$null = New-Item -ItemType Directory -Force $Destination
$dist = (Resolve-Path $Destination).Path
$stage = Join-Path $dist ("bed-package-" + [guid]::NewGuid().ToString("N"))
$package = Join-Path $stage "bEd"
try {
    $null = New-Item -ItemType Directory -Force $package
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $package "bed.exe")
    Copy-Item -Recurse resources (Join-Path $package "resources")
    Copy-Item -Recurse resources/queries (Join-Path $package "queries")
    Copy-Item -Recurse LICENSES (Join-Path $package "LICENSES")
    Copy-Item LICENSE, NOTICE, PORTING.md -Destination $package
    python scripts/collect-licenses.py $package
    if ($LASTEXITCODE -ne 0) { throw 'License notice collection failed.' }
    python scripts/package-remote-helpers.py copy --destination $package
    if ($LASTEXITCODE -ne 0) { throw 'Remote helper bundle validation or copy failed.' }
    foreach ($asset in @("resources/config/bed.json", "resources/fonts/SourceCodePro-Regular.ttf", "resources/fonts/Emoji.ttf", "resources/icons/bed.png", "resources/icons/bed.ico", "queries/rs.scm", "LICENSES/terminal-adapter-BSL-1.1.txt")) {
        if (!(Test-Path (Join-Path $package $asset))) { throw "Missing packaged asset: $asset" }
    }
    $archive = Join-Path $dist "bEd-$version-windows-x64.zip"
    Compress-Archive -Path $package -DestinationPath $archive -Force
    $output = Join-Path $dist "bEd"
    if (Test-Path $output) { Remove-Item -LiteralPath $output -Recurse -Force }
    Move-Item -LiteralPath $package -Destination $output
    Write-Output $archive
} finally {
    if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
