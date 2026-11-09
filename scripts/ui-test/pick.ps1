param([string]$Path, [int]$ProcId, [switch]$Cancel)
# Maneja SOLO el diálogo nativo "Abrir" de la ventana NovaScore del proceso $ProcId.
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$dlg = $null
for ($i = 0; $i -lt 30 -and -not $dlg; $i++) {
  $main = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ProcessId -eq $ProcId -and $_.Current.Name -eq 'NovaScore' } | Select-Object -First 1
  if ($main) { $dlg = $main.FindFirst([System.Windows.Automation.TreeScope]::Children, (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, 'Abrir'))) }
  if (-not $dlg) { Start-Sleep -Milliseconds 300 }
}
if (-not $dlg) { "no apareció el diálogo"; exit 1 }
$all = $dlg.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
if ($Cancel) {
  $c = $all | Where-Object { $_.Current.AutomationId -eq '2' -and $_.Current.ClassName -eq 'Button' } | Select-Object -First 1
  if (-not $c) { "no encuentro Cancelar"; exit 4 }
  $c.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
  "diálogo cancelado"; exit 0
}
$edit = $all | Where-Object { $_.Current.AutomationId -eq '1148' -and $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Edit' } | Select-Object -First 1
if (-not $edit) { "no encuentro el campo Nombre"; exit 2 }
$edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Path)
$btns = @($all | Where-Object { $_.Current.AutomationId -eq '1' -and $_.Current.ClassName -eq 'Button' })
if ($btns.Count -eq 0) { "no encuentro el botón Abrir"; exit 3 }
$real = $btns | Where-Object { $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Button' } | Select-Object -First 1
$pat = $null
if ($real -and $real.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pat)) { $pat.Invoke() }
else {
  # Sin botón invocable: Entrar en el campo, solo si el foco está en el diálogo de NovaScore
  $edit.SetFocus()
  Start-Sleep -Milliseconds 300
  $f = [System.Windows.Automation.AutomationElement]::FocusedElement
  if ($f.Current.ProcessId -ne $ProcId) { "el foco no está en NovaScore; no envío Entrar"; exit 6 }
  Add-Type -AssemblyName System.Windows.Forms
  [System.Windows.Forms.SendKeys]::SendWait("{ENTER}")
}
"ruta confirmada: $Path"
