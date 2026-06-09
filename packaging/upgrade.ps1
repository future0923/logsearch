param(
  [string]$Version = "latest"
)

$ErrorActionPreference = "Stop"

$GithubOwner = $env:LOG_SEARCH_GITHUB_OWNER
if ([string]::IsNullOrWhiteSpace($GithubOwner)) { $GithubOwner = "future0923" }
$GithubRepo = $env:LOG_SEARCH_GITHUB_REPO
if ([string]::IsNullOrWhiteSpace($GithubRepo)) { $GithubRepo = "logsearch" }
$GiteeOwner = $env:LOG_SEARCH_GITEE_OWNER
if ([string]::IsNullOrWhiteSpace($GiteeOwner)) { $GiteeOwner = "future94" }
$GiteeRepo = $env:LOG_SEARCH_GITEE_REPO
if ([string]::IsNullOrWhiteSpace($GiteeRepo)) { $GiteeRepo = "logsearch" }
$Mirror = $env:LOG_SEARCH_MIRROR
if ([string]::IsNullOrWhiteSpace($Mirror)) { $Mirror = "auto" }

$AppDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) ("log-search-upgrade-" + [System.Guid]::NewGuid().ToString("N"))

function Fail($Message) {
  throw "upgrade failed: $Message"
}

function Normalize-Arch {
  if ($env:LOG_SEARCH_ARCH) { $value = $env:LOG_SEARCH_ARCH } else { $value = $env:PROCESSOR_ARCHITECTURE }
  switch -Regex ($value) {
    "^(AMD64|x64|amd64)$" { "amd64"; return }
    "^(ARM64|arm64|aarch64)$" { "arm64"; return }
    default { Fail "unsupported architecture: $value" }
  }
}

function Invoke-Download($Url, $Target) {
  if ($Url.StartsWith("file://")) {
    Copy-Item -LiteralPath $Url.Substring(7) -Destination $Target -Force
    return
  }
  Invoke-WebRequest -Uri $Url -OutFile $Target -UseBasicParsing
}

function Read-Url($Url) {
  $target = Join-Path $WorkDir "response.json"
  Invoke-Download $Url $target
  Get-Content -LiteralPath $target -Raw
}

function Resolve-Latest-Version {
  if ($env:LOG_SEARCH_LATEST_URL) {
    $json = Read-Url $env:LOG_SEARCH_LATEST_URL | ConvertFrom-Json
    if ($json.tag_name) { return $json.tag_name }
  }

  if ($Mirror -eq "auto" -or $Mirror -eq "gitee") {
    try {
      $json = Read-Url "https://gitee.com/api/v5/repos/$GiteeOwner/$GiteeRepo/releases/latest" | ConvertFrom-Json
      if ($json.tag_name) { return $json.tag_name }
    } catch {
      if ($Mirror -eq "gitee") { throw }
    }
  }

  if ($Mirror -eq "auto" -or $Mirror -eq "github") {
    $json = Read-Url "https://api.github.com/repos/$GithubOwner/$GithubRepo/releases/latest" | ConvertFrom-Json
    if ($json.tag_name) { return $json.tag_name }
  }

  Fail "could not resolve latest release version"
}

function Download-Release($Tag, $Asset, $Target) {
  $versionWithoutV = $Tag -replace "^v", ""
  if ($env:LOG_SEARCH_UPGRADE_BASE_URL) {
    Invoke-Download (($env:LOG_SEARCH_UPGRADE_BASE_URL.TrimEnd("/")) + "/$versionWithoutV/$Asset") $Target
    return
  }

  if ($Mirror -eq "auto" -or $Mirror -eq "gitee") {
    try {
      Invoke-Download "https://gitee.com/$GiteeOwner/$GiteeRepo/releases/download/$Tag/$Asset" $Target
      return
    } catch {
      if ($Mirror -eq "gitee") { throw }
    }
  }

  if ($Mirror -eq "auto" -or $Mirror -eq "github") {
    Invoke-Download "https://github.com/$GithubOwner/$GithubRepo/releases/download/$Tag/$Asset" $Target
    return
  }

  Fail "could not download release asset: $Asset"
}

function Get-ArgValue($CommandLine, $Option) {
  $pattern = [regex]::Escape($Option) + "\s+([^\s]+)"
  $match = [regex]::Match($CommandLine, $pattern)
  if ($match.Success) { return $match.Groups[1].Value }
  return ""
}

function Get-RunningCommandLine {
  $escapedApp = [regex]::Escape($AppDir)
  $process = Get-CimInstance Win32_Process -Filter "name = 'log-search.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ExecutablePath -and ($_.ExecutablePath -match $escapedApp) } |
    Select-Object -First 1
  if ($process) { return $process.CommandLine }
  return ""
}

function Capture-RuntimeState {
  if ($env:LOG_SEARCH_PREVIOUS_CMDLINE) {
    $commandLine = $env:LOG_SEARCH_PREVIOUS_CMDLINE
  } else {
    $commandLine = Get-RunningCommandLine
  }

  $config = Get-ArgValue $commandLine "--config"
  $staticDir = Get-ArgValue $commandLine "--static-dir"
  if ([string]::IsNullOrWhiteSpace($config)) { $config = Join-Path $AppDir "config.toml" }
  if ([string]::IsNullOrWhiteSpace($staticDir)) { $staticDir = Join-Path $AppDir "frontend" }

  [pscustomobject]@{
    Config = $config
    StaticDir = $staticDir
  }
}

function Stop-Current {
  Get-CimInstance Win32_Process -Filter "name = 'log-search.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ExecutablePath -and ($_.ExecutablePath -like "$AppDir*") } |
    ForEach-Object {
      Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
    }
}

function Backup-Current {
  $backupDir = Join-Path $AppDir ("backups\" + (Get-Date -Format "yyyyMMdd-HHmmss"))
  New-Item -ItemType Directory -Path $backupDir -Force | Out-Null
  foreach ($name in @("config.toml", "log-search.exe", "frontend", "upgrade.ps1", "README.txt", "README-windows.txt")) {
    $source = Join-Path $AppDir $name
    if (Test-Path -LiteralPath $source) {
      Copy-Item -LiteralPath $source -Destination $backupDir -Recurse -Force
    }
  }
  $backupDir
}

function Sync-Release($ReleaseDir) {
  $currentUpgrade = Join-Path $WorkDir "current-upgrade.ps1"
  $appUpgrade = Join-Path $AppDir "upgrade.ps1"
  if (Test-Path -LiteralPath $appUpgrade) {
    Copy-Item -LiteralPath $appUpgrade -Destination $currentUpgrade -Force
  }

  foreach ($item in Get-ChildItem -LiteralPath $AppDir -Force) {
    if ($item.Name -in @("config.toml", "data", "logs", "run", "backups")) { continue }
    Remove-Item -LiteralPath $item.FullName -Recurse -Force
  }

  foreach ($item in Get-ChildItem -LiteralPath $ReleaseDir -Force) {
    if ($item.Name -in @("config.toml", "data", "logs", "run", "backups")) { continue }
    Copy-Item -LiteralPath $item.FullName -Destination $AppDir -Recurse -Force
  }

  if (-not (Test-Path -LiteralPath $appUpgrade) -and (Test-Path -LiteralPath $currentUpgrade)) {
    Copy-Item -LiteralPath $currentUpgrade -Destination $appUpgrade -Force
  }

  Copy-Item -LiteralPath (Join-Path $ReleaseDir "config.toml") -Destination (Join-Path $AppDir "config.toml.new") -Force
}

function Start-Current($Runtime) {
  $exe = Join-Path $AppDir "log-search.exe"
  if (-not (Test-Path -LiteralPath $exe)) { Fail "log-search.exe not found after upgrade" }
  Start-Process -FilePath $exe -ArgumentList @("--config", $Runtime.Config, "--static-dir", $Runtime.StaticDir) -WorkingDirectory $AppDir
}

try {
  New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null
  $arch = Normalize-Arch
  $tag = $Version
  if ($tag -eq "latest") { $tag = Resolve-Latest-Version }
  $versionWithoutV = $tag -replace "^v", ""
  $asset = "log-search_${versionWithoutV}_windows_${arch}.zip"
  $archive = Join-Path $WorkDir $asset
  $extractDir = Join-Path $WorkDir "extracted"

  Write-Host "Log Search upgrade"
  Write-Host "App dir: $AppDir"
  Write-Host "Version: $tag"
  Write-Host "Asset: $asset"

  $runtime = Capture-RuntimeState
  Write-Host "Detected config: $($runtime.Config)"

  Download-Release $tag $asset $archive
  Expand-Archive -LiteralPath $archive -DestinationPath $extractDir -Force
  $releaseDir = Get-ChildItem -LiteralPath $extractDir -Directory | Select-Object -First 1
  if (-not $releaseDir) { Fail "release archive did not contain a directory" }
  if (-not (Test-Path -LiteralPath (Join-Path $releaseDir.FullName "log-search.exe"))) { Fail "new release missing log-search.exe" }
  if (-not (Test-Path -LiteralPath (Join-Path $releaseDir.FullName "frontend"))) { Fail "new release missing frontend directory" }

  $backup = Backup-Current
  Write-Host "Backup: $backup"
  Stop-Current
  Sync-Release $releaseDir.FullName
  Start-Current $runtime

  Write-Host "Upgrade finished."
  Write-Host "User config kept: $(Join-Path $AppDir "config.toml")"
  Write-Host "New sample config: $(Join-Path $AppDir "config.toml.new")"
} finally {
  if (Test-Path -LiteralPath $WorkDir) {
    Remove-Item -LiteralPath $WorkDir -Recurse -Force
  }
}
