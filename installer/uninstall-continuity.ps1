$ErrorActionPreference = "Stop"

$uninstallRoots = @(
    "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
    "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*",
    "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*"
)

$app = Get-ItemProperty $uninstallRoots -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq "Continuity" } |
    Select-Object -First 1

if ($null -eq $app) {
    Write-Host "Continuity is not installed through the MSI installer."
    exit 1
}

$productCode = $null
if ($app.PSChildName -match "^\{[0-9A-Fa-f-]{36}\}$") {
    $productCode = $app.PSChildName
} elseif ($app.UninstallString -match "\{[0-9A-Fa-f-]{36}\}") {
    $productCode = $Matches[0]
}

if ($null -eq $productCode) {
    Write-Host "Could not find Continuity's MSI product code."
    Write-Host "Uninstall string: $($app.UninstallString)"
    exit 1
}

Write-Host "Uninstalling Continuity ($productCode)..."
$process = Start-Process -FilePath "msiexec.exe" -ArgumentList @("/x", $productCode) -Wait -PassThru
exit $process.ExitCode
