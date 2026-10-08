# Install gulms on Windows: download the latest release, verify its checksum, and add it to your PATH.
#
#   irm https://raw.githubusercontent.com/omsingh02/gulms/main/install.ps1 | iex
#
# Environment:
#   GULMS_VERSION      install a specific tag, e.g. v0.1.0 (default: the latest release)
#   GULMS_INSTALL_DIR  where to put gulms.exe (default: %LOCALAPPDATA%\Programs\gulms)
#   GULMS_BASE_URL     read release files from this URL or folder instead of GitHub (used by tests)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'omsingh02/gulms'
$Target = 'x86_64-pc-windows-msvc'
$InstallDir = if ($env:GULMS_INSTALL_DIR) { $env:GULMS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\gulms' }
$BaseUrl = $env:GULMS_BASE_URL

$Version = $env:GULMS_VERSION
if (-not $Version) {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'gulms-installer' }
    $Version = $release.tag_name
}
if (-not $BaseUrl) { $BaseUrl = "https://github.com/$Repo/releases/download/$Version" }

$Archive = "gulms-$Version-$Target.zip"
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("gulms-install-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null

function Get-ReleaseFile($name) {
    $dest = Join-Path $Tmp $name
    if (Test-Path -LiteralPath $BaseUrl -PathType Container) {
        Copy-Item -LiteralPath (Join-Path $BaseUrl $name) -Destination $dest
    } else {
        Invoke-WebRequest -Uri "$BaseUrl/$name" -OutFile $dest -UseBasicParsing
    }
    return $dest
}

try {
    Write-Host "Installing gulms $Version ($Target)"
    $zip = Get-ReleaseFile $Archive
    $sums = Get-ReleaseFile 'SHA256SUMS'

    $line = Get-Content $sums | Where-Object { $_ -match ("\s" + [regex]::Escape($Archive) + "$") } | Select-Object -First 1
    if (-not $line) { throw "no checksum listed for $Archive" }
    $expected = ($line -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLower()
    if ($expected -ne $actual) { throw "checksum mismatch for $Archive; refusing to install" }

    Expand-Archive -LiteralPath $zip -DestinationPath $Tmp -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $Tmp "gulms-$Version-$Target\gulms.exe") -Destination (Join-Path $InstallDir 'gulms.exe') -Force
    Write-Host "Installed to $(Join-Path $InstallDir 'gulms.exe')"

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $InstallDir), 'User')
        Write-Host "Added $InstallDir to your PATH. Open a new terminal to use 'gulms'."
    }
    Write-Host ""
    Write-Host "Next: run 'gulms' to sign in and get started."
} finally {
    Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
