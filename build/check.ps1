# Copies the sources from this PC to the build machine and runs build/check.sh there
# (server tests and the app's wasm compile check). Nothing heavy runs on this PC.
#   powershell -ExecutionPolicy Bypass -File build\check.ps1 [all|server|app]
# Files deleted here are not deleted on the server; remove them there by hand.
param([ValidateSet("all", "server", "app")][string]$What = "all")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$key = "$HOME\.ssh\chat_deploy"
$hostName = "mboot@goofyfiles.duckdns.org"
$archive = Join-Path $env:TEMP "goofychat-sources.tgz"

Push-Location $root
try {
    & tar.exe -czf $archive `
        --exclude=target --exclude=dist --exclude=gen --exclude=icons --exclude=.gradle --exclude=build/outputs `
        --exclude=.env --exclude=*.apk --exclude=*.log `
        app proto server deploy docs build README.md
    if ($LASTEXITCODE -ne 0) { throw "tar failed" }
} finally { Pop-Location }

& scp.exe -q -i $key $archive "${hostName}:/tmp/goofychat-sources.tgz"
if ($LASTEXITCODE -ne 0) { throw "scp failed" }
Remove-Item $archive
& ssh.exe -i $key $hostName "cd ~/goofychat && tar -xzf /tmp/goofychat-sources.tgz && rm /tmp/goofychat-sources.tgz && sh build/check.sh $What"
exit $LASTEXITCODE
