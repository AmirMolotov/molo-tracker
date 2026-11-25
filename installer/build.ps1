param([string]$Compiler)

$ErrorActionPreference = 'Stop'
if (!$Compiler) {
    $Compiler = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
}
if (!$Compiler) {
    $Compiler = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (!$Compiler) {
    throw 'Install Inno Setup 6 or pass the path to ISCC.exe with -Compiler.'
}
$Compiler = (Get-Item -LiteralPath $Compiler).FullName

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $metadata = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo package metadata.' }
    $package = ($metadata | ConvertFrom-Json).packages | Where-Object name -eq 'molo-tracker'

    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Molo Tracker release build failed.' }

    & $Compiler "/DAppVersion=$($package.version)" (Join-Path $PSScriptRoot 'MoloTracker.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Molo Tracker installer build failed.' }
}
finally {
    Pop-Location
}
