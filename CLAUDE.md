# NovaScore — contexto para Claude Code

Editor de partituras libre (GPL-3.0-or-later), alternativa a MuseScore Studio. Tauri 2 + Rust + TypeScript (Solid).
Idioma de trabajo con el usuario: **español**. El detalle completo está en `docs/HANDOFF.md` (léelo primero en una sesión nueva).

## Estado en una línea
Editor funcional y verificado contra MuseScore 4.6: notación completa básica, 4 voces, tuplets, cambios de compás, repeticiones/casillas,
ligaduras de expresión, reguladores, dinámicas, articulaciones, octavas, pedal, trinos, percusión, mezclador, SoundFont, PDF, MIDI,
MusicXML y lectura/escritura de `.mscz`/`.mscx` (v2–v4). **IA local:** audio→partitura, acordes y OMR (Audiveris externo) hechos; asistente simbólico (acompañamiento, armonización de la melodía y progresiones, secuencias con modulación e inversión, cadencia y revisión); cambios de tonalidad a mitad de pieza (`Score.key_changes`) hecho; OMR probado con Audiveris 5.11 real; no hay modelo generativo. Ver `docs/HANDOFF.md` §7.

## Mapa del código (`app/`)
- `crates/nova-core/` — núcleo en Rust, sin UI. `lib.rs` modelo + edición + undo; `io.rs` sonidos/MIDI/MusicXML; `mscx.rs` lector MuseScore; `mscx_write.rs` escritor MuseScore.
  `examples/ms_roundtrip.rs`, `examples/mscx_info.rs` y `examples/eval_harmonize.rs` (armonización contra progresiones sintéticas) son herramientas de verificación.
- `crates/nova-audio/` — audio→notas con Basic Pitch (Apache-2.0, ONNX de 225 KB incluido en el binario, `ort`) y `piano.rs`: modelo de piano de alta precisión (ByteDance, pesos CC-BY-4.0, ONNX de 98 MB descargable, GPU por DirectML; ver `docs/MODELOS.md`). Umbrales calibrados: `docs/CALIBRACION.md`. `nova-core/src/quantize.rs` — notas en segundos → `Score` (tonalidad, pentagramas, voces, grupos irregulares 3:2, 5:4 y 6:4); `nova-core/src/assist.rs` — asistente (acompañamiento desde el cifrado y revisión de tesitura/voces/paralelas); `nova-core/src/tab.rs` — tablatura (cuerda/traste, técnicas `Event.tech`; réplica en `src/tab.ts`; entrada por traste con F2); `nova-core/src/chords.rs` — detección de acordes (simbólica, por pulsos; `examples/eval_chords`); `nova-core/src/beats.rs` — seguimiento de pulso (mapa de tempos variable, DP tipo Ellis). Comando Tauri `import_audio` (`src-tauri/src/audio.rs`, hilo aparte) + botón «Importar audio» con diálogo (tempo, compás, sensibilidad). `novascore.exe audio.wav` también lo abre por línea de órdenes.
- `src-tauri/` — comandos Tauri (una función por operación; estado en `Mutex<Document>`), `pdf.rs` (svg2pdf), `omr.rs` (partitura impresa → MusicXML con Audiveris como proceso externo, sin empaquetar) y `deskew.rs` (endereza escaneos y fotos torcidos antes del OMR).
- `src/` — interfaz Solid: `App.tsx` (estado, teclado, barras), `ScoreView.tsx` (dibujo SVG con Bravura), `layout.ts` (maquetación), `playback.ts` (SpessaSynth), `pdfExport.tsx`.
- `samples/` — MusicXML de ejemplo, uno por funcionalidad. `scripts/` — utilidades (empaquetar, PDF→PNG).

## Comandos
```
cd app && npm install && npm run tauri dev          # ejecutar
cd app/crates/nova-core && cargo test               # 174 tests del núcleo
cd app/crates/nova-audio && cargo test              # 5 tests; audio→notas (Basic Pitch/ONNX). Ejemplos: audio2notes, audio2score, roundtrip (mide F1 sobre samples/)
cd app && npx tsc --noEmit                          # tipos del frontend
cd app && npx tauri build --debug --no-bundle       # exe de depuración (src-tauri/target/debug/novascore.exe)
NOVASCORE_EXPORT_PDF=salida.pdf novascore.exe archivo.(musicxml|mscz|nova|mid|pdf|png…)   # exporta a PDF sin interacción (sirve para comprobar el dibujo)
```
Verificación contra MuseScore real (necesita MuseScore 4 instalado; variable `MUSESCORE` si no está en `C:\Program Files\MuseScore 4\bin`):
`cd app/crates/nova-core && cargo run --release --example ms_roundtrip -- entrada salida.mscz` → debe imprimir `IDENTICAL NOTES + IDENTICAL SPANNERS + IDENTICAL MARKS`.

## Invariantes y trampas (aprendidas)
- 480 ticks por negra (`PPQ`). `Event.ticks` es la duración **sonora**; dentro de un tuplet la escrita es `notated_ticks()`.
- Las notas guardan la **altura real (sonora)**. Bajo una octava (8va…) se dibujan desplazadas (`displayScore` en TS), y MuseScore guarda la altura *escrita* (el lector/escritor convierten).
- Los spanners (ligaduras, reguladores, octavas, pedal, trinos, casillas) se anclan a **tiempo** `(compás, tick)`, no a notas.
- `sounds()` despliega repeticiones/casillas (`Score::playback_order`); `Sound.start` es tiempo de reproducción.
- Contenido de MuseScore 4 no entendido se conserva literal en `Event.extras`/`Measure.extras`/`Part.ms`/`Score.ms_style` (solo archivos v4).
- Compatibilidad hacia atrás: todos los campos nuevos del modelo llevan `#[serde(default, skip_serializing_if…)]`; un `.nova` antiguo debe abrir.
- Windows: PowerShell 5.1 `Set-Content -Encoding utf8` mete BOM y rompe los JSON de Tauri. En Git Bash, `node -e "…"` con barras invertidas se corrompe: escribe el script a un `.js` y ejecútalo.
- `src-tauri/target` llega a ~11 GB: no copiarlo, no versionarlo.
- Las muestras reales del usuario (Canon, Chopin) estaban en su carpeta Descargas y **no** forman parte del repositorio.

## Cómo trabajar con este usuario
- Avanzar por módulos en el orden acordado; tras cada bloque: tests, comprobación visual (PDF) y, si toca formatos, verificación con MuseScore.
- Informar con honestidad qué se probó y qué no (p. ej. botones de la UI sin probar a mano), límites y pendientes.
- Antes de decisiones que cambian el rumbo (licencias, empaquetado de modelos de IA), preguntar con una recomendación clara.
- Licencia objetivo GPL-3; preferir componentes libres y procesamiento local. Verificar la licencia de **pesos** de modelos, no solo del código.
