# Prueba de la entrada por traste con PULSACIONES REALES (SendKeys sobre la ventana con el foco). Lanzar con powershell.exe 5.1
# desde la raíz del repositorio y con el exe de depuración compilado; escribe el estado antes y después.
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName Microsoft.VisualBasic
Add-Type @"
using System; using System.Runtime.InteropServices;
public class Fg { [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow(); [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h); [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid); }
"@
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9333 --remote-allow-origins=*"
$p = Start-Process app\src-tauri\target\debug\novascore.exe -ArgumentList 'D:\ProjectsIA\NovaScore\samples\tablatura.musicxml' -PassThru
Start-Sleep 8
[Microsoft.VisualBasic.Interaction]::AppActivate($p.Id)
Start-Sleep 1
[Fg]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Start-Sleep 1
$fg = [Fg]::GetForegroundWindow(); $fpid = 0; [Fg]::GetWindowThreadProcessId($fg, [ref]$fpid) | Out-Null
"foco en NovaScore: " + ($fpid -eq $p.Id)
$dump = @"
(async()=>{const s=await window.__TAURI_INTERNALS__.invoke('get_score'); const pi=s.parts.findIndex(p=>p.tuning); const m=s.parts[pi].measures; return JSON.stringify({parte:pi, compases:m.length, m0:m[0].events.map(e=>e.kind==='note'?{n:e.pitches.map(q=>q.step+':'+q.alter+':'+q.octave).join('+'),t:e.ticks,s:e.strings}:{r:e.ticks}), estado:document.querySelector('.statusbar, .status, footer').innerText.split('\n').pop()})})()
"@
"ANTES: " + (node scripts/ui-test/cdp.js eval $dump)
function K($keys, $ms=350) { [System.Windows.Forms.SendKeys]::SendWait($keys); Start-Sleep -Milliseconds $ms }
K "{F2}" 600
"tras F2: " + (node scripts/ui-test/cdp.js eval "document.querySelector('.statusbar, .status, footer').innerText.split('\n').pop()")
K "3" 1200          # cuerda 1, traste 3
K "{DOWN}{DOWN}" 300 # cuerda 3
K "5" 1200
K "1" 80; K "2" 1200 # dos cifras seguidas: traste 12
K "]" 300           # duracion mayor
K "{UP}" 300
K "0" 1200
K "r" 800           # silencio
K "{F2}" 600
"DESPUES: " + (node scripts/ui-test/cdp.js eval $dump)
node scripts/ui-test/cdp.js shot $env:TEMP\fret_real.png
Stop-Process -Id $p.Id -Force
