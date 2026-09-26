# Traces attempted network activity of the app through launch, checkbox changes, the
# Scan-to-Clean handoff, Clean, the automatic rescan, and close, then runs curl as a positive control.
# Run elevated: trace-network.ps1 -Exe <cc-cleaner.exe> -Dir <output folder>
param([string]$Exe, [string]$Dir)
$etl = "$Dir\trace.etl"
Remove-Item "$Dir\trace.etl", "$Dir\trace.xml" -ErrorAction SilentlyContinue
logman stop cleanernet -ets 2>$null | Out-Null
logman start cleanernet -ets -o $etl -p Microsoft-Windows-Winsock-AFD 0xFFFFFFFFFFFFFFFF 0xff | Out-Null
logman update trace cleanernet -ets -p Microsoft-Windows-Kernel-Network 0xFFFFFFFFFFFFFFFF 0xff | Out-Null
logman update trace cleanernet -ets -p Microsoft-Windows-DNS-Client 0xFFFFFFFFFFFFFFFF 0xff | Out-Null
logman update trace cleanernet -ets -p Microsoft-Windows-WinHttp 0xFFFFFFFFFFFFFFFF 0xff | Out-Null

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$app = Start-Process $Exe -PassThru
Start-Sleep -Seconds 3
$root = $A::FromHandle($app.MainWindowHandle)
# Waits up to 30 s for an enabled button whose name starts with $prefix, then presses it.
function Invoke-Named($prefix) {
    for ($i = 0; $i -lt 60; $i++) {
        $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        $e = $all | Where-Object { $_.Current.Name -like "$prefix*" -and $_.Current.IsEnabled } | Select-Object -First 1
        if ($e) { $e.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); "pressed '$($e.Current.Name)'"; return }
        Start-Sleep -Milliseconds 500
    }
    "never enabled: $prefix"
}
# Untick and tick a checkbox, then Clean while the slow unticked Scan is still going.
$box = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, (New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, 'Recycle Bin')))
$toggle = $box.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
$toggle.Toggle(); $toggle.Toggle()
Invoke-Named "Clean"
Start-Sleep -Seconds 40       # Clean, then the automatic rescan
$app.CloseMainWindow() | Out-Null
$app.WaitForExit(10000) | Out-Null

$control = Start-Process curl.exe -ArgumentList '-s', '-o', 'NUL', 'https://example.com' -PassThru -Wait -WindowStyle Hidden
logman stop cleanernet -ets | Out-Null
tracerpt $etl -o "$Dir\trace.xml" -of XML -y | Out-Null

$events = [regex]::Matches((Get-Content "$Dir\trace.xml" -Raw), '(?s)<Event .*?</Event>') | ForEach-Object Value
# ETW stamps kernel receive work with whichever thread it interrupted, so attribute
# Kernel-Network events by their PID field and skip AFD receive indications.
function Owned($id) {
    $events | Where-Object {
        if ($_ -match 'Microsoft-Windows-Kernel-Network') { $_ -match "Name=`"PID`">\s*$id<" }
        else { $_ -match "ProcessID=`"$id`"" -and $_ -notmatch 'AfdDataIndication' }
    }
}
function Summary($id) {
    $mine = @(Owned $id)
    $tasks = $mine | ForEach-Object { if ($_ -match '<Task>(\w+)') { $Matches[1] } } | Group-Object | ForEach-Object { "$($_.Name) x$($_.Count)" }
    "$($mine.Count) owned network events [$($tasks -join ', ')]"
}
"trace: $($events.Count) events"
"app pid $($app.Id): $(Summary $app.Id); exit code $($app.ExitCode)"
"curl control pid $($control.Id): $(Summary $control.Id)"
