# Drives the Cleaner window through UI Automation for verification runs.
# Run with Windows PowerShell from the repo root:
#   powershell -NoProfile -ExecutionPolicy Bypass -File .claude\skills\verify-cleaner\scripts\verify.ps1 <command> [args]
# Commands: launch [-Exe <path>] | doctor | tree [-Out <file>] | toggle <name> | invoke <name>
#           | wait-scan | wait-name <name> | shot <file> | stop
# State for the running instance lives in target\verify\current.json; evidence lives in its run folder.
param(
    [Parameter(Mandatory, Position = 0)]
    [ValidateSet('launch', 'doctor', 'tree', 'toggle', 'invoke', 'wait-scan', 'wait-name', 'shot', 'stop')]
    [string]$Command,
    [Parameter(Position = 1)][string]$Name,
    [string]$Exe = 'target\debug\cc-cleaner.exe',
    [string]$Out,
    [int]$Timeout = 600
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
$VerifyDir = Join-Path $Repo 'target\verify'
$StateFile = Join-Path $VerifyDir 'current.json'
$Selection = Join-Path $env:APPDATA 'cc-cleaner-at-home\selection.json'
$Ellipsis = [char]0x2026

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
# Row statuses are GPUI aria_descriptions, exposed only as UIA FullDescription, which the
# managed client cannot read. This minimal COM interop reads them by accessible name.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
namespace VerifyUia {
  [ComImport, Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IUIAutomation {
    void _CompareElements(); void _CompareRuntimeIds(); void _GetRootElement();
    IUIAutomationElement ElementFromHandle(IntPtr hwnd);
    void _ElementFromPoint(); void _GetFocusedElement(); void _GetRootElementBuildCache();
    void _ElementFromHandleBuildCache(); void _ElementFromPointBuildCache(); void _GetFocusedElementBuildCache();
    void _CreateTreeWalker(); void _ControlViewWalker(); void _ContentViewWalker(); void _RawViewWalker();
    void _RawViewCondition(); void _ControlViewCondition(); void _ContentViewCondition(); void _CreateCacheRequest();
    [return: MarshalAs(UnmanagedType.IUnknown)] object CreateTrueCondition();
  }
  [ComImport, Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IUIAutomationElement {
    void _SetFocus(); void _GetRuntimeId(); void _FindFirst();
    IUIAutomationElementArray FindAll(int scope, [MarshalAs(UnmanagedType.IUnknown)] object condition);
    void _FindFirstBuildCache(); void _FindAllBuildCache(); void _BuildUpdatedCache();
    [return: MarshalAs(UnmanagedType.Struct)] object GetCurrentPropertyValue(int id);
  }
  [ComImport, Guid("14314595-b4bc-4055-95f2-58f2e42c9855"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IUIAutomationElementArray { int Length(); IUIAutomationElement GetElement(int i); }
  [ComImport, Guid("ff48dba4-60ef-4201-aa87-54103eef594e")] class CUIAutomation {}
  public static class Native {
    // Accessible name -> UIA FullDescription (GPUI aria_description) for every descendant.
    public static Dictionary<string, string> Descriptions(IntPtr hwnd) {
      var uia = (IUIAutomation)new CUIAutomation();
      var all = uia.ElementFromHandle(hwnd).FindAll(4, uia.CreateTrueCondition());
      var map = new Dictionary<string, string>();
      for (int i = 0; i < all.Length(); i++) {
        var e = all.GetElement(i);
        var name = e.GetCurrentPropertyValue(30005) as string;
        var desc = e.GetCurrentPropertyValue(30159) as string;
        if (!string.IsNullOrEmpty(name) && !string.IsNullOrEmpty(desc)) map[name] = desc;
      }
      return map;
    }
  }
}
'@
$A = [System.Windows.Automation.AutomationElement]
$Scope = [System.Windows.Automation.TreeScope]::Descendants

function Get-State {
    if (-not (Test-Path $StateFile)) { throw 'No verification instance is recorded. Run launch first.' }
    Get-Content $StateFile -Raw | ConvertFrom-Json
}

function Get-App($state) {
    $app = Get-Process -Id $state.pid -ErrorAction SilentlyContinue
    if (-not $app -or $app.Path -ne $state.exe) { throw "Recorded Cleaner pid $($state.pid) is not running. Run stop to restore state." }
    $app
}

function Get-Root($app) {
    $app.Refresh()
    if ($app.MainWindowHandle -eq 0) { throw 'Cleaner has no window.' }
    $A::FromHandle($app.MainWindowHandle)
}

# Exact accessible name, or a -like pattern when it contains '*' (e.g. 'Clean approximately*').
function Find-Named($root, [string]$name) {
    if ($name.Contains('*')) {
        return $root.FindAll($Scope, [System.Windows.Automation.Condition]::TrueCondition) |
            Where-Object { $_.Current.Name -like $name } | Select-Object -First 1
    }
    $condition = [System.Windows.Automation.PropertyCondition]::new($A::NameProperty, $name)
    $root.FindFirst($Scope, $condition)
}

function Get-Names($root) {
    @($root.FindAll($Scope, [System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object { $_.Current.Name })
}

function Wait-Until([string]$step, $app, [scriptblock]$ready) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    do {
        if ($app.HasExited) { throw "Cleaner exited during $step." }
        if (& $ready) { return }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out after $Timeout s during $step."
}

# Idle means the Scan button is enabled and no Selection load or save is pending.
function Wait-Idle($app, $root) {
    Wait-Until 'Scan and Selection idle' $app {
        $scan = Find-Named $root 'Scan'
        $names = Get-Names $root
        $scan -and $scan.Current.IsEnabled -and
            -not ($names -contains "Saving Selection$Ellipsis" -or $names -contains "Loading Selection$Ellipsis")
    }
}

function Dump($e, $depth, $descriptions) {
    $c = $e.Current
    $line = ('  ' * $depth) + $c.ControlType.ProgrammaticName.Replace('ControlType.', '') + " '" + $c.Name + "'"
    if (-not $c.IsEnabled) { $line += ' [disabled]' }
    $tp = $null
    if ($e.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$tp)) { $line += ' toggle=' + $tp.Current.ToggleState }
    if ($descriptions.ContainsKey($c.Name)) { $line += ' desc="' + $descriptions[$c.Name] + '"' }
    $line
    $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
    $child = $walker.GetFirstChild($e)
    while ($child) { Dump $child ($depth + 1) $descriptions; $child = $walker.GetNextSibling($child) }
}

function Resolve-Evidence($state, [string]$file) {
    if ([System.IO.Path]::IsPathRooted($file)) { return $file }
    Join-Path $state.run $file
}

switch ($Command) {
    'launch' {
        if (Test-Path $StateFile) { throw "A verification instance is already recorded in $StateFile. Run stop first." }
        # One Cleaner per Windows account: a second launch only shows "Cleaner is already running."
        $other = Get-Process cc-cleaner -ErrorAction SilentlyContinue
        if ($other) { throw "Another Cleaner is running (pid $($other.Id -join ', ')). It is not ours; ask the user to close it." }
        $exePath = (Resolve-Path (Join-Path $Repo $Exe)).Path
        $run = Join-Path $VerifyDir (Get-Date -Format 'yyyyMMdd-HHmmss')
        $null = New-Item -ItemType Directory -Path $run -Force
        # The app writes the real Selection file; keep the user's copy to restore on stop.
        $backup = $null
        if (Test-Path $Selection) {
            $backup = Join-Path $run 'selection.before.json'
            Copy-Item $Selection $backup
        }
        $app = Start-Process -FilePath $exePath -PassThru -WindowStyle Normal
        [pscustomobject]@{ pid = $app.Id; exe = $exePath; run = $run; backup = $backup } |
            ConvertTo-Json | Set-Content $StateFile
        Wait-Until 'window creation' $app { $app.Refresh(); $app.MainWindowHandle -ne 0 }
        Wait-Idle $app (Get-Root $app)
        "READY pid=$($app.Id) run=$run"
    }
    'doctor' {
        $state = Get-State
        $app = Get-App $state
        $root = Get-Root $app
        $names = Get-Names $root
        $scan = Find-Named $root 'Scan'
        $newest = Get-ChildItem (Join-Path $Repo 'src'), (Join-Path $Repo 'crates') -Recurse -File |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        $built = (Get-Item $state.exe).LastWriteTime
        "pid:      $($app.Id) ($($state.exe))"
        "window:   '$($root.Current.Name)'"
        "build:    $built$(if ($newest.LastWriteTime -gt $built) { "  STALE: $($newest.Name) changed after the build" })"
        "scan:     $(if ($scan -and $scan.Current.IsEnabled) { 'idle' } else { 'busy or missing' })"
        "summary:  $($names | Where-Object { $_ -like 'Selected estimate:*' } | Select-Object -First 1)"
        "clean:    $($names | Where-Object { $_ -like 'Clean*' -and $_ -notlike 'Clean permanently*' } | Select-Object -First 1)"
        "evidence: $($state.run)"
    }
    'tree' {
        $state = Get-State
        $app = Get-App $state
        $root = Get-Root $app
        $lines = Dump $root 0 ([VerifyUia.Native]::Descriptions($app.MainWindowHandle))
        if ($Out) { $lines | Set-Content (Resolve-Evidence $state $Out) -Encoding UTF8 }
        $lines
    }
    'toggle' {
        $state = Get-State
        $app = Get-App $state
        $root = Get-Root $app
        $box = Find-Named $root $Name
        if (-not $box) { throw "No control named '$Name'." }
        if (-not $box.Current.IsEnabled) { throw "'$Name' is disabled." }
        $pattern = $box.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
        $before = $pattern.Current.ToggleState
        $pattern.Toggle()
        Wait-Until "$Name toggle" $app { $pattern.Current.ToggleState -ne $before }
        Wait-Until 'Selection save' $app { -not ((Get-Names $root) -contains "Saving Selection$Ellipsis") }
        "$Name toggle=$($pattern.Current.ToggleState)"
    }
    'invoke' {
        $state = Get-State
        $root = Get-Root (Get-App $state)
        $button = Find-Named $root $Name
        if (-not $button) { throw "No control named '$Name'." }
        if (-not $button.Current.IsEnabled) { throw "'$Name' is disabled." }
        $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
        "invoked $Name"
    }
    'wait-scan' {
        $state = Get-State
        $app = Get-App $state
        # Give a just-invoked Scan time to disable the button before waiting for idle.
        Start-Sleep -Milliseconds 300
        Wait-Idle $app (Get-Root $app)
        'idle'
    }
    'wait-name' {
        $state = Get-State
        $app = Get-App $state
        $root = Get-Root $app
        Wait-Until "'$Name' to appear" $app { $null -ne (Find-Named $root $Name) }
        "found $Name"
    }
    'shot' {
        $state = Get-State
        $app = Get-App $state
        $root = Get-Root $app
        Add-Type -AssemblyName System.Drawing
        Add-Type -Namespace Verify -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, int flags);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, int[] rect);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, int[] rect, int size);
'@
        # PrintWindow with PW_RENDERFULLCONTENT renders the window even when it is covered or
        # unfocused. Crop to DWM's frame bounds to drop the invisible resize borders.
        $h = $app.MainWindowHandle
        $w = [int[]]::new(4); $null = [Verify.Win]::GetWindowRect($h, $w)
        $f = [int[]]::new(4); $null = [Verify.Win]::DwmGetWindowAttribute($h, 9, $f, 16)
        $full = [System.Drawing.Bitmap]::new($w[2] - $w[0], $w[3] - $w[1])
        $g = [System.Drawing.Graphics]::FromImage($full)
        $hdc = $g.GetHdc()
        $printed = [Verify.Win]::PrintWindow($h, $hdc, 2)
        $g.ReleaseHdc($hdc); $g.Dispose()
        if (-not $printed) { throw 'PrintWindow failed.' }
        $crop = [System.Drawing.Rectangle]::new($f[0] - $w[0], $f[1] - $w[1], $f[2] - $f[0], $f[3] - $f[1])
        $bmp = $full.Clone($crop, $full.PixelFormat)
        $full.Dispose()
        $path = Resolve-Evidence $state $Name
        $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        "saved $path"
    }
    'stop' {
        $state = Get-State
        $app = Get-Process -Id $state.pid -ErrorAction SilentlyContinue
        if ($app -and $app.Path -eq $state.exe) {
            $null = $app.CloseMainWindow()
            if (-not $app.WaitForExit(30000)) { Stop-Process -Id $state.pid -Force }
        }
        if (Test-Path $Selection) { Copy-Item $Selection (Join-Path $state.run 'selection.after.json') }
        if ($state.backup) { Copy-Item $state.backup $Selection -Force }
        elseif (Test-Path $Selection) { Remove-Item $Selection }
        Remove-Item $StateFile
        "stopped pid=$($state.pid); Selection restored; evidence kept in $($state.run)"
    }
}
