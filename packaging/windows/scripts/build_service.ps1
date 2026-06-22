$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path "$PSScriptRoot/../../../.."
Push-Location $RepoRoot
cargo build --release -p busytok-service
Pop-Location
