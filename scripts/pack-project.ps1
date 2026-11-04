# Empaqueta el proyecto para pasarlo a otro equipo, sin carpetas de compilación.
# Uso (desde cualquier carpeta):  powershell -File scripts\pack-project.ps1 [-Out C:\ruta\NovaScore-transfer.zip]
param([string]$Out = (Join-Path (Split-Path (Resolve-Path "$PSScriptRoot\..") -Parent) "NovaScore-transfer.zip"))
$root = Resolve-Path "$PSScriptRoot\.."
$parent = Split-Path $root -Parent
$name = Split-Path $root -Leaf
if (Test-Path $Out) { Remove-Item $Out -Force }
# bsdtar viene con Windows 10/11
tar -a -c -f $Out --exclude=target --exclude=node_modules --exclude=dist --exclude=gen --exclude="*.log" -C $parent $name
"{0}: {1:N1} MB" -f $Out, ((Get-Item $Out).Length / 1MB)
