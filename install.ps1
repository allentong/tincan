# Install the latest tincan release binary into $env:TINCAN_INSTALL_DIR (default %LOCALAPPDATA%\tincan\bin).
$ErrorActionPreference = 'Stop'

$repo = 'allentong/tincan'
$dir = if ($env:TINCAN_INSTALL_DIR) { $env:TINCAN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'tincan\bin' }

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  'AMD64' { 'x86_64' }
  'ARM64' { 'aarch64' }
  default { throw "tincan: unsupported CPU $env:PROCESSOR_ARCHITECTURE" }
}

$asset = "tincan-$arch-pc-windows-msvc.zip"
$url = "https://github.com/$repo/releases/latest/download/$asset"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Write-Host "Downloading $url"
  Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile (Join-Path $tmp $asset)
  Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile (Join-Path $tmp "$asset.sha256")
  $want = (Get-Content (Join-Path $tmp "$asset.sha256") -Raw).Split(' ')[0].Trim().ToLower()
  $got = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLower()
  if ($want -ne $got) { throw "tincan: checksum mismatch for $asset" }
  $gh = Get-Command gh -ErrorAction SilentlyContinue
  if ($gh) {
    & gh auth status --hostname github.com 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) {
      & gh attestation verify (Join-Path $tmp $asset) --repo $repo --signer-workflow "$repo/.github/workflows/release.yml" | Out-Null
      if ($LASTEXITCODE -ne 0) { throw "tincan: GitHub build provenance verification failed for $asset" }
      Write-Host "Verified GitHub build provenance for $asset"
    } else {
      Write-Host 'GitHub CLI is not authenticated; verified checksum only (see README for provenance verification).'
    }
  } else {
    Write-Host 'GitHub CLI is unavailable; verified checksum only (see README for provenance verification).'
  }
  Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  Copy-Item -Force (Join-Path $tmp 'tincan.exe') (Join-Path $dir 'tincan.exe')
  $version = & (Join-Path $dir 'tincan.exe') --version
  Write-Host "Installed $version to $dir\tincan.exe"
  # The skill ships in the binary: put it where Claude Code, Codex and Grok look for skills.
  & (Join-Path $dir 'tincan.exe') install-skills | Out-Null
  if ($LASTEXITCODE -eq 0) { Write-Host "Installed the tincan skill for Claude Code, Codex and Grok" }
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not (($env:Path + ';' + $userPath).Split(';') -contains $dir)) {
    Write-Host "Add $dir to your PATH:"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$dir;`" + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User')"
  }
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
