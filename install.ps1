$ErrorActionPreference = "Stop"

$repo = "Fabian2000/delegate-shell"
$asset = "dgsh-windows-x86_64.zip"
$installDir = "$env:USERPROFILE\.dgsh\bin"

Write-Host "Fetching latest release..."
$release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
$tag = $release.tag_name

if (-not $tag) {
    Write-Host "Failed to fetch latest release."
    exit 1
}

Write-Host "Installing dgsh $tag for Windows x86_64..."

$url = "https://github.com/$repo/releases/download/$tag/$asset"
$tmpDir = New-TemporaryFile | ForEach-Object { Remove-Item $_; New-Item -ItemType Directory -Path $_ }
$zipPath = Join-Path $tmpDir $asset

Invoke-WebRequest -Uri $url -OutFile $zipPath

if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}

Expand-Archive -Path $zipPath -DestinationPath $tmpDir -Force
Move-Item -Path (Join-Path $tmpDir "dgsh.exe") -Destination (Join-Path $installDir "dgsh.exe") -Force

Remove-Item -Recurse -Force $tmpDir

# Add to PATH if not already there
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($currentPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$currentPath;$installDir", "User")
    Write-Host "Added $installDir to PATH (restart terminal to take effect)."
}

Write-Host "dgsh $tag installed to $installDir\dgsh.exe"
Write-Host "Run 'dgsh' to start the REPL or 'dgsh script.dgsh' to run a script."
