# Prints the Cleaner window's UI Automation tree, which is what Narrator reads.
# -Toggle <name> or -Invoke <name> acts on a control first. Run elevated.
param([string]$Toggle, [string]$Invoke, [int]$Wait = 0)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$h = (Get-Process cc-cleaner | Select-Object -First 1).MainWindowHandle
$root = $A::FromHandle($h)
function Find($name) {
    $c = New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, $name)
    $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $c)
}
if ($Toggle) { (Find $Toggle).GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle() }
if ($Invoke) { (Find $Invoke).GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() }
if ($Wait) { Start-Sleep -Milliseconds $Wait }
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
function Dump($e, $depth) {
    $c = $e.Current
    $line = ('  ' * $depth) + $c.ControlType.ProgrammaticName.Replace('ControlType.', '') + " '" + $c.Name + "'"
    if (-not $c.IsEnabled) { $line += ' [disabled]' }
    if ($c.HasKeyboardFocus) { $line += ' [focused]' }
    if ($c.IsKeyboardFocusable) { $line += ' [focusable]' }
    $tp = $null
    if ($e.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$tp)) { $line += ' toggle=' + $tp.Current.ToggleState }
    if ($c.HelpText) { $line += ' help="' + $c.HelpText + '"' }
    $line
    $child = $walker.GetFirstChild($e)
    while ($child) { Dump $child ($depth + 1); $child = $walker.GetNextSibling($child) }
}
Dump $root 0
