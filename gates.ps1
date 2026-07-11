# Full quality gates on Windows — the same checks CI runs. Green here = shippable.
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

cargo test --all-targets
if ($LASTEXITCODE -ne 0) { exit 1 }

cargo clippy --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit 1 }

cargo fmt --check
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host "ALL GATES GREEN"
