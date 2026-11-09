# Prueba de la interfaz real (Windows)

Herramientas para manejar el `.exe` de depuración sin pulsar a mano. Solo para desarrollo.

1. Compilar: `cd app && npx tauri build --debug --no-bundle`.
2. Lanzar con el puerto de depuración de WebView2 abierto (PowerShell):
   `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9333 --remote-allow-origins=*"; Start-Process app\src-tauri\target\debug\novascore.exe -PassThru`
3. `node scripts/ui-test/cdp.js eval "<JS>"` ejecuta JavaScript en la interfaz (p. ej. pulsar un botón o leer la barra de estado) y
   `node scripts/ui-test/cdp.js shot salida.png` hace una captura.
4. El selector de archivos nativo («Abrir») no se ve desde CDP: `scripts/ui-test/pick.ps1 -Path <archivo> -ProcId <PID>`
   (ejecutar con PowerShell 7, `pwsh`) escribe la ruta y confirma, o con `-Cancel` lo cancela. Solo toca el diálogo del proceso indicado.
5. Cerrar la app al terminar (solo el PID lanzado).

Se usó para probar «Importar audio»: selector, diálogo, mensaje «Transcribiendo…», resultado, segundo audio con otras opciones,
cancelar, archivo que no es audio, y que la ventana sigue respondiendo durante el cálculo.
