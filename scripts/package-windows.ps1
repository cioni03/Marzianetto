param(
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')]
    [string]$Target = 'x86_64-pc-windows-msvc'
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path $PSScriptRoot -Parent
Push-Location $Root
try {
    & rustup target add $Target
    if ($LASTEXITCODE -ne 0) { throw 'Installazione target Rust fallita.' }
    & cargo build --locked --release --target $Target
    if ($LASTEXITCODE -ne 0) { throw 'Build fallita. Verifica Visual Studio C++ Build Tools e Windows SDK.' }
    $Out = Join-Path $Root "dist/windows/$Target"
    New-Item -ItemType Directory -Force -Path $Out | Out-Null
    Copy-Item "target/$Target/release/marzianetto.exe" "$Out/marzianetto.exe"
    Copy-Item 'README.md', 'README.en.md', 'BUILDING.md' $Out
    Compress-Archive -Path "$Out/marzianetto.exe", "$Out/README.md", "$Out/README.en.md", "$Out/BUILDING.md" -DestinationPath "$Out/marzianetto-$Target.zip" -Force
    Write-Host "Creati EXE e ZIP in $Out"
    Write-Host 'Il menu contestuale si installa dalla UI. Non sono necessari privilegi amministrativi.'
} finally {
    Pop-Location
}
