$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path "$PSScriptRoot/../../../.."
$SrcTauri = "$RepoRoot/apps/gui/src-tauri"
$BinDir = "$SrcTauri/binaries"
$Target = "$RepoRoot/target/release"

function Copy-Sidecar($name) {
    $src = "$Target/$name.exe"
    $dst = "$BinDir/$name-x86_64-pc-windows-msvc.exe"
    if (-not (Test-Path $src)) { throw "Release build missing: $src. Run build_service.ps1 + build_cli.ps1 first." }
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item -Force $src $dst
    Write-Host "Bundled sidecar: $dst"
}
function Invoke-BundleHelpers { Copy-Sidecar "busytok-service"; Copy-Sidecar "busytok" }
Export-ModuleMember -Function Invoke-BundleHelpers
