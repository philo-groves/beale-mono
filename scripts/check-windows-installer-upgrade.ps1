param(
  [Parameter(Mandatory = $true)][string]$CurrentInstaller,
  [Parameter(Mandatory = $true)][string]$ExpectedVersion,
  [Parameter(Mandatory = $true)][string]$ExpectedPublisher
)

$ErrorActionPreference = 'Stop'
$releasesJson = gh release list --limit 100 --json tagName,isDraft,isPrerelease
if ($LASTEXITCODE -ne 0) { throw 'Published releases could not be listed.' }
$previousTag = @($releasesJson | ConvertFrom-Json | Where-Object {
  -not $_.isDraft -and -not $_.isPrerelease -and $_.tagName -match '^v\d+\.\d+\.\d+$' -and
  [version]$_.tagName.Substring(1) -lt [version]$ExpectedVersion
} | Sort-Object { [version]$_.tagName.Substring(1) } -Descending |
  Select-Object -First 1 -ExpandProperty tagName)
if ($previousTag.Count -ne 1) {
  throw 'The previous published release could not be identified.'
}
$previousVersion = $previousTag[0].Substring(1)
$previousInstaller = Join-Path $env:RUNNER_TEMP "Beale-Setup-$previousVersion.exe"
gh release download $previousTag[0] --pattern "Beale-Setup-$previousVersion.exe" --dir $env:RUNNER_TEMP
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $previousInstaller)) {
  throw 'The previous Windows installer could not be downloaded.'
}

function Assert-SignedInstaller([string]$path) {
  $signature = Get-AuthenticodeSignature -LiteralPath $path
  if ($signature.Status -ne 'Valid') { throw "Invalid installer signature: $path" }
  $publisher = $signature.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
  if ($publisher -ne $ExpectedPublisher -or -not $signature.TimeStamperCertificate) {
    throw "Unexpected installer signer or missing timestamp: $path"
  }
}

function Installed-Beale {
  $entries = @(Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq 'Beale' })
  if ($entries.Count -ne 1) { throw "Expected one per-user Beale installation; found $($entries.Count)." }
  if ($entries[0].UninstallString -notmatch '^"([^"]+\.exe)"') {
    throw 'The Beale uninstaller path is missing from the per-user registry entry.'
  }
  $exe = Join-Path (Split-Path -Parent $Matches[1]) 'Beale.exe'
  if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'The installed Beale executable is missing.' }
  return @{ Version = $entries[0].DisplayVersion; Exe = $exe }
}

Assert-SignedInstaller $previousInstaller
Assert-SignedInstaller $CurrentInstaller
$first = Start-Process -FilePath $previousInstaller -ArgumentList @('/S', '/currentuser') -Wait -PassThru
if ($first.ExitCode -ne 0) { throw "The previous per-user installer exited with $($first.ExitCode)." }
$installed = Installed-Beale
if ($installed.Version -ne $previousVersion) { throw "Expected previous version $previousVersion; found $($installed.Version)." }

# The former app-server supervisor also ran from Beale.exe. Keep one process
# alive to exercise the installer's replacement path used by existing clients.
$env:ELECTRON_RUN_AS_NODE = '1'
try {
  $holder = Start-Process -FilePath $installed.Exe -ArgumentList @('-e', 'setInterval(()=>{},1000)') -PassThru
} finally {
  Remove-Item Env:ELECTRON_RUN_AS_NODE -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 1
if ($holder.HasExited) { throw 'The previous Beale executable did not stay running for the upgrade test.' }

try {
  $upgrade = Start-Process -FilePath $CurrentInstaller -ArgumentList @('--updated', '/S', '--force-run') -PassThru
  if (-not $upgrade.WaitForExit(120000)) {
    Stop-Process -Id $upgrade.Id -Force -ErrorAction SilentlyContinue
    throw 'The silent upgrade did not finish within two minutes.'
  }
  if ($upgrade.ExitCode -ne 0) { throw "The silent upgrade exited with $($upgrade.ExitCode)." }
  $installed = Installed-Beale
  if ($installed.Version -ne $ExpectedVersion) {
    throw "Expected upgraded version $ExpectedVersion; found $($installed.Version)."
  }
} finally {
  if (-not $holder.HasExited) { Stop-Process -Id $holder.Id -Force -ErrorAction SilentlyContinue }
}

Write-Host "Per-user silent upgrade from $previousVersion to $ExpectedVersion kept Beale.exe installed."
