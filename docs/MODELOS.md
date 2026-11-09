# Modelos de IA de NovaScore

| Modelo | Uso | Tamaño | Licencia (código / pesos) | Dónde está |
|---|---|---|---|---|
| Basic Pitch (Spotify) | Audio → notas, cualquier instrumento | 225 KB | Apache-2.0 / Apache-2.0 | Incluido en el binario (`nova-audio/models/`) |
| High-resolution Piano Transcription (Kong et al., ByteDance) | Audio → notas de piano, con velocidad | 98 MB (ONNX fp32) | Apache-2.0 / **CC-BY-4.0** (Zenodo 4034264: exige atribución) | **Descarga bajo demanda**; no va en el repositorio |

## Modelo de piano
- Fuente: https://zenodo.org/records/4034264 (`CRNN_note_F1=0.9677_pedal_F1=0.9186.pth`, 172 MB). Código: https://github.com/bytedance/piano_transcription.
- Conversión: `scripts/export-piano-onnx.py` (PyTorch ≥ 2.x; solo se necesita una vez). Se exporta la red de notas (sin pedal) con STFT y mel dentro del ONNX: entrada `audio [N,160000]` @16 kHz, salidas `onset/offset/frame/velocity [N,1001,88]`. Verificado contra PyTorch: diferencia máxima 7e-7.
- Ejecución: `nova_audio::piano` (ONNX Runtime). Dispositivo `Auto` = GPU por DirectML (cualquier GPU de Windows) con vuelta a CPU. Chopin (268 s): 37 s en GPU (RTX 3050) frente a 79 s en CPU, notas idénticas.
- Ubicación: `NOVASCORE_PIANO_MODEL` o `<datos de la app>/modelos/piano.onnx`. El diálogo «Importar audio» habilita el motor «Piano» solo si existe.
- Atribución obligatoria (CC-BY-4.0): «High-resolution Piano Transcription with Pedals by Regressing Onsets and Offsets Times», Q. Kong, B. Li, X. Song, Y. Wan, Y. Wang; los pesos convertidos a ONNX son una obra derivada y deben indicarlo.

## Descarga bajo demanda
- Alojado como *release* `models-v1` de GitHub: https://github.com/Gorgogito/NovaScore/releases/tag/models-v1 (`piano.onnx`, 98 684 797 bytes, SHA-256 `2b43528170ada21dc95da555b39f9926239ebd5caec061daac8614d36eb59546`). Las notas del release llevan la atribución CC-BY-4.0.
- `audio::download_verified` descarga a `piano.onnx.part`, comprueba tamaño y SHA-256 y solo entonces renombra: nunca queda un modelo a medias ni corrupto. Comando `download_piano_model` (emite `piano-download {done,total}`); el diálogo «Importar audio» muestra el botón y el progreso, y activa el motor al terminar.
- Probado: descarga real (test `--ignored download_real_model`, 10 s) y flujo completo en la interfaz (descargar → motor piano → transcribir Chopin, 1408 notas).
- `ureq` está fijado a 3.1.4 en `Cargo.lock` (la 3.5.1 pide `webpki-roots` 1.0.10, no disponible en este índice).

- **Cancelar y borrar** (2026-11-09): `cancel_piano_download` activa una bandera que `download_verified` comprueba en cada bloque (borra el `.part` y devuelve «Descarga cancelada.»); `delete_piano_model` borra el modelo de la carpeta de datos (no toca rutas de `NOVASCORE_PIANO_MODEL`). En el diálogo: «Cancelar descarga» mientras se descarga y «Borrar modelo de piano» con confirmación en dos pasos. Probado en la interfaz real: borrar (No conserva, Sí borra), cancelar al 27 % (sin archivos a medias, sin error visible), volver a descargar (mismo SHA-256). Tests: servidor HTTP local con descarga correcta, cancelada y de suma incorrecta; borrado idempotente.
- **`DirectML.dll` en el instalador** (2026-11-09): `src-tauri/build.rs` copia la DLL que `ort` descarga (primero `target/<perfil>/`, luego la caché `%LOCALAPPDATA%/ort.pyke.io`) a `src-tauri/resources/` (ignorada por git) y `bundle.resources` la pone junto al ejecutable. Comprobado en el script del instalador NSIS (`File ... DirectML.dll` y su desinstalación); instalador de 51 MB. **Instalador probado** (2026-11-09, Windows 11, RTX 3050, instalación silenciosa `/S` por usuario en `%LOCALAPPDATA%\NovaScore`, que pone `novascore.exe`, `DirectML.dll` y `uninstall.exe`): el *Canon* (268 s) con el motor de piano tarda 30-33 s, con uso de GPU del proceso de hasta 37-93 % (contador `GPU Engine`) y el mismo resultado que la compilación de depuración (tempo 68,2, 4/4 por defecto). La lista de módulos del proceso muestra `d3d12.dll` del sistema y `DirectML.dll` 1.15.4 **de la carpeta de instalación**, es decir, se carga la empaquetada. **Control sin la DLL empaquetada**: también usa la GPU (93 %) y tarda 27 s, porque Windows 11 trae su propio `DirectML.dll` en System32; por tanto el empaquetado no es imprescindible en Windows 11 actual, pero fija la versión (1.15.4) y cubre Windows 10 antiguos cuyo DirectML del sistema sea más viejo (**no probado**). Sin ninguna DirectML disponible no se probó la vuelta a CPU con el instalador (sí con `Device::Cpu` en los ejemplos: 79 s). Licencia: Microsoft publica `Microsoft.AI.DirectML` como paquete redistribuible y su repositorio es MIT, pero **no he podido leer el texto de licencia del binario del paquete NuGet**: verificarlo antes de publicar un instalador.

## Pendiente
- Instalar el NSIS en una máquina limpia / Windows 10 antiguo (sin la DLL de sistema reciente) y comprobar GPU y caída a CPU.
- Probar en otra máquina/GPU (solo RTX 3050 probada) y sin conexión (mensaje de error).
