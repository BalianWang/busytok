$ErrorActionPreference = "Stop"
trap {
    Write-Host "##vso[task.logissue type=error] packaging failed: $_"
    Write-Host "##vso[task.logissue type=error] at line $($_.InvocationInfo.ScriptLineNumber): $($_.InvocationInfo.Line)"
    exit 1
}
$RepoRoot = Resolve-Path "$PSScriptRoot/../../../.."
Push-Location $RepoRoot
& "$PSScriptRoot/build_service.ps1"
& "$PSScriptRoot/build_cli.ps1"
Import-Module "$PSScriptRoot/_bundle_helpers.ps1"
Invoke-BundleHelpers
Push-Location "$RepoRoot/apps/gui"
pnpm exec tauri build --bundles nsis
Pop-Location
Pop-Location
Write-Host "Installer at: $RepoRoot/apps/gui/src-tauri/target/release/bundle/nsis/"
