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

## Pendiente
- Alojar el ONNX para la descarga (p. ej. un *release* de GitHub) y añadir el descargador con barra de progreso y SHA-256. Mientras tanto se instala a mano.
- Empaquetar `DirectML.dll` junto al ejecutable (ort la copia a `target/`; falta incluirla en el instalador).
