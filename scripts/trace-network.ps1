# Trace the supported, non-destructive UI workflow and use curl as a positive control.
# Clean, its Scan handoff, and its automatic rescan are not verified by this script.
# Run elevated: trace-network.ps1 -Exe <cc-cleaner.exe> -Dir <output folder>
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Dir
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Network verification requires an elevated PowerShell session; no steps were verified.'
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if (Get-Process cc-cleaner -ErrorAction SilentlyContinue) {
    throw 'Close the existing Cleaner instance before tracing; no steps were verified.'
}
$null = New-Item -ItemType Directory -Path $Dir -Force
$Dir = (Resolve-Path -LiteralPath $Dir).Path
$etl = Join-Path $Dir 'trace.etl'
$xml = Join-Path $Dir 'trace.xml'
$session = "cleanernet-$PID"
$app = $null
$tracing = $false

function Invoke-CheckedNative([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE; verification is incomplete." }
}

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
function Find-Named([string]$Name) {
    $condition = [System.Windows.Automation.PropertyCondition]::new($A::NameProperty, $Name)
    $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
}
function Wait-Until([string]$Step, [scriptblock]$Ready) {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        if ($app.HasExited) { throw "Cleaner exited during $Step; verification is incomplete." }
        if (& $Ready) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out during $Step; verification is incomplete."
}
function Wait-Selection {
    Wait-Until 'Selection persistence' {
        $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        $names = @($all | ForEach-Object { $_.Current.Name })
        $failure = $names | Where-Object { $_ -like 'Selection *' } | Select-Object -First 1
        if ($failure) { throw "Selection interaction failed: $failure" }
        -not ($names -contains 'Saving Selection...' -or $names -contains 'Loading Selection...')
    }
}
function Wait-Scan {
    Wait-Until 'Scan completion' {
        $button = Find-Named 'Rescan'
        $button -and $button.Current.IsEnabled
    }
    Wait-Selection
}

try {
    Invoke-CheckedNative logman @('start', $session, '-ets', '-o', $etl, '-ow', '-p', 'Microsoft-Windows-Winsock-AFD', '0xFFFFFFFFFFFFFFFF', '0xff')
    $tracing = $true
    foreach ($provider in 'Microsoft-Windows-Kernel-Network', 'Microsoft-Windows-DNS-Client', 'Microsoft-Windows-WinHttp') {
        Invoke-CheckedNative logman @('update', 'trace', $session, '-ets', '-p', $provider, '0xFFFFFFFFFFFFFFFF', '0xff')
    }
    $app = Start-Process -FilePath $Exe -PassThru -WindowStyle Normal
    Wait-Until 'window creation' { $app.Refresh(); $app.MainWindowHandle -ne 0 }
    $root = $A::FromHandle($app.MainWindowHandle)
    Wait-Scan
    'VERIFIED: launch and initial Scan finished.'

    $toggled = 0
    foreach ($name in 'User temp', 'Windows temp') {
        $box = Find-Named $name
        if (-not $box) {
            Write-Warning "NOT VERIFIED: $name checkbox changes (Target is not displayed)."
            continue
        }
        if (-not $box.Current.IsEnabled) { throw "The $name checkbox is disabled; verification is incomplete." }
        $pattern = $box.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
        $original = $pattern.Current.ToggleState
        try {
            $pattern.Toggle()
            Wait-Until "$name toggle" { $pattern.Current.ToggleState -ne $original }
            Wait-Selection
        } finally {
            # Restore the user's original choice even when an assertion fails.
            if (-not $app.HasExited -and $pattern.Current.ToggleState -ne $original) {
                $pattern.Toggle()
                Wait-Until "$name restoration" { $pattern.Current.ToggleState -eq $original }
                Wait-Selection
            }
        }
        $toggled++
        "VERIFIED: $name toggled and restored."
    }
    if ($toggled -eq 0) { throw 'No supported Target checkbox was available; verification is incomplete.' }

    $rescan = Find-Named 'Rescan'
    $rescan.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
    # Let the asynchronous UI action run before observing completion.
    Start-Sleep -Milliseconds 250
    Wait-Scan
    'VERIFIED: manual Rescan finished.'

    $clean = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition) |
        Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -like 'Clean*' } |
        Select-Object -First 1
    if (-not $clean) { throw 'Clean control was not found; verification is incomplete.' }
    $reason = if ($clean.Current.IsEnabled) { 'this script exercises only non-destructive controls' } else { 'Clean is disabled in this build' }
    Write-Warning "NOT VERIFIED: Clean, Scan-to-Clean handoff, and automatic post-Clean rescan ($reason)."

    if (-not $app.CloseMainWindow()) { throw 'Could not request app close; verification is incomplete.' }
    if (-not $app.WaitForExit(30000)) { throw 'Cleaner did not close; verification is incomplete.' }
    if ($app.ExitCode -ne 0) { throw "Cleaner exited with code $($app.ExitCode); verification is incomplete." }
    'VERIFIED: app close.'
    $control = Start-Process curl.exe -ArgumentList '--fail', '--silent', '--show-error', '--max-time', '30', '-o', 'NUL', 'https://example.com' -PassThru -Wait -WindowStyle Hidden
    if ($control.ExitCode -ne 0) { throw "curl positive control failed with exit code $($control.ExitCode); verification is incomplete." }
} finally {
    try {
        if ($null -ne $app -and -not $app.HasExited) {
            $null = $app.CloseMainWindow()
            if (-not $app.WaitForExit(10000)) { Write-Warning 'Cleaner is still running; close it manually.' }
        }
    } finally {
        if ($tracing) { Invoke-CheckedNative logman @('stop', $session, '-ets') }
    }
}

Invoke-CheckedNative tracerpt @($etl, '-o', $xml, '-of', 'XML', '-y')
$events = [regex]::Matches((Get-Content -LiteralPath $xml -Raw), '(?s)<Event .*?</Event>') | ForEach-Object Value
# ETW stamps kernel receive work with whichever thread it interrupted, so attribute
# Kernel-Network events by their PID field and skip AFD receive indications.
function Owned($id) {
    $events | Where-Object {
        if ($_ -match 'Microsoft-Windows-Kernel-Network') { $_ -match "Name=`"PID`">\s*$id<" }
        else { $_ -match "ProcessID=`"$id`"" -and $_ -notmatch 'AfdDataIndication' }
    }
}
$appEvents = @(Owned $app.Id)
$controlEvents = @(Owned $control.Id)
if ($controlEvents.Count -eq 0) { throw 'No curl network events captured; the trace cannot verify network inactivity.' }
if ($appEvents.Count -ne 0) { throw "Network verification failed: Cleaner owns $($appEvents.Count) network events. Inspect $xml." }
"PASS for exercised steps only: launch, displayed Target checkbox changes, manual Rescan, and close."
"app pid $($app.Id): 0 owned network events; curl control pid $($control.Id): $($controlEvents.Count) owned network events."
"NOT VERIFIED: Clean, Scan-to-Clean handoff, automatic post-Clean rescan. Trace: $xml"
