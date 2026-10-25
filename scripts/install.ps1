# Installs the headless CC Manager server (ccm-server.exe) from GitHub releases.
#
#   irm https://raw.githubusercontent.com/joneswapple/cc-Mannger/main/scripts/install.ps1 | iex
#
# Environment: CCM_VERSION (default latest), CCM_INSTALL_DIR
# (default %LOCALAPPDATA%\Programs\cc-manager). For the desktop app, use the
# NSIS installer from the release page instead.
$ErrorActionPreference = "Stop"

$Repo = "joneswapple/cc-Mannger"
$Version = if ($env:CCM_VERSION) { $env:CCM_VERSION } else { "latest" }
$Dir = if ($env:CCM_INSTALL_DIR) { $env:CCM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\cc-manager" }

if ([Environment]::Is64BitOperatingSystem -eq $false) { throw "64-bit Windows is required" }
$Asset = "ccm-server-x86_64-pc-windows-msvc.zip"
$Url = if ($Version -eq "latest") {
  "https://github.com/$Repo/releases/latest/download/$Asset"
} else {
  "https://github.com/$Repo/releases/download/$Version/$Asset"
}

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
  Write-Host "downloading $Url"
  Invoke-WebRequest -Uri $Url -OutFile (Join-Path $Tmp $Asset) -UseBasicParsing
  Expand-Archive -Path (Join-Path $Tmp $Asset) -DestinationPath $Tmp -Force
  New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  Copy-Item (Join-Path $Tmp "ccm-server.exe") (Join-Path $Dir "ccm-server.exe") -Force
} finally {
  Remove-Item -Recurse -Force $Tmp
}

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($UserPath -split ";" | Where-Object { $_ -eq $Dir })) {
  [Environment]::SetEnvironmentVariable("Path", "$UserPath;$Dir", "User")
  Write-Host "added $Dir to your user PATH (open a new terminal)"
}
Write-Host "installed $(Join-Path $Dir 'ccm-server.exe')"
Write-Host "run: ccm-server serve   (the admin token is printed on first start)"
