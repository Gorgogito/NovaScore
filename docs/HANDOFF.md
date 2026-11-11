# NovaScore — documento de traspaso

Fecha: 2026-11-03. Objetivo: continuar el proyecto en otro equipo (más RAM y GPU) con la parte de **IA local**.

## 1. Qué es
Editor de partituras libre y gratuito (GPL-3.0-or-later) que busca igualar a MuseScore Studio. Procesamiento 100 % local, sin servicios de pago.
Decisiones acordadas con el usuario: licencia GPL-3 (se puede reutilizar código de MuseScore, no habrá versión cerrada) · stack Tauri 2 + Rust + TypeScript/Solid · prioridad: flujo de edición y composición antes que compatibilidad (la compatibilidad `.mscz` ya está hecha).

## 2. Estado funcional (todo implementado y probado)
| Área | Contenido |
|---|---|
| Edición | Entrada por teclado y ratón, acordes, ligaduras de prolongación, deshacer/rehacer (instantáneas), selección de rango (Mayús+flechas), transposición, zoom |
| Notación | Claves sol/fa/percusión, armaduras, **cambios de compás**, 4 voces por pentagrama (cabezas desplazadas), tuplets (3:2, 5:4, 6:4, 2:3, 4:3 y los importados), vigas, alteraciones por compás, percusión GM con cabezas en cruz |
| Expresión | Ligaduras de expresión, reguladores, dinámicas (ppp–fff, fp, sf, sfz, rfz), articulaciones (staccato, staccatissimo, tenuto, acento, marcato, portato), octavas 8va/8vb/15ma/15mb, pedal, trinos, **repeticiones y casillas** |
| Sonido | SpessaSynth (Apache-2.0) + SoundFont MuseScore_General (MIT); velocidad por dinámicas/hairpins/articulaciones, pedal (CC64), trinos, repeticiones desplegadas; mezclador (volumen, mute, solo), 25 instrumentos GM |
| Formatos | `.nova` (JSON propio) · MusicXML in/out · MIDI in/out (import cuantizado a semicorcheas) · PDF vectorial multipágina (svg2pdf) · **MuseScore `.mscz/.mscx` lectura v2–v4 y escritura v4.60** |

Pendiente conocido (backlog sin IA): saltos D.C./D.S./segno/coda (se pierden al guardar), mordentes/grupetos, MIDI import sin tresillos ni varias voces, 7:4 y otros tuplets, vista/edición de letra y texto, edición manual del cifrado de acordes y su exportación (MusicXML `<harmony>`, MuseScore `Harmony`; la detección y el dibujo ya existen: botón «Detectar acordes», `Score.chords`), partes/extractos, transposición de instrumentos (se muestran en altura real), ligadura entre >2 sistemas, tablatura, plugins.

## 3. Arquitectura
```
app/
  crates/nova-core/   (Rust, sin UI)   lib.rs   modelo, edición, undo, repeticiones (playback_order), spanners
                                        io.rs    sounds()/pedals(), MIDI, MusicXML (import+export)
                                        mscx.rs  lector MuseScore 2/3/4     mscx_write.rs  escritor MuseScore 4.60
  src-tauri/          (Rust)           lib.rs   comandos Tauri, AppState(Mutex<Document>)   pdf.rs  SVG→PDF
  src/                (Solid/TS)       App.tsx  estado+teclado+barras   ScoreView.tsx  dibujo SVG (Bravura)
                                        layout.ts  maquetación           playback.ts  SpessaSynth   pdfExport.tsx  paginación A4
```
Flujo: la UI llama a un comando Tauri por operación → el núcleo edita `Document` → devuelve el `Score` completo (JSON) → la UI lo redibuja. El núcleo es la única fuente de verdad.

### Modelo (resumen; `crates/nova-core/src/lib.rs`)
`Score { title, composer, time, time_changes[], fifths, tempo, parts[], ms_style }` · `Part { name, clef, program, volume, muted, solo, percussion, ms, spanners[], measures[] }` ·
`Measure { events (voz 1), voices (voces 2–4), extras, start_repeat, end_repeat }` · `Event { start, ticks, content: Note{pitches[],tie}|Rest, tuplet, extras, dynamic, articulations }` ·
`Spanner { kind: Slur|Crescendo|Decrescendo|Ottava*|Pedal|Trill|Volta, voice, start/end: SpPos{measure,tick}, endings }`.
Todo campo nuevo es opcional con `serde(default)`: los `.nova` antiguos deben seguir abriéndose.

### Decisiones técnicas que conviene recordar
- 480 ticks/negra. Duración sonora vs escrita en tuplets (`notated_ticks`). Alturas **sonoras**; las octavas se dibujan desplazadas y MuseScore guarda la altura escrita.
- Spanners anclados a tiempo, no a notas (sobreviven a ediciones; el escritor los ajusta a notas reales).
- El contenido de MuseScore 4 que no entendemos se copia literal (extras) → ida y vuelta sin pérdidas de dinámicas/letra/etc. (los archivos 2.x/3.x no admiten copia literal).
- Canales MIDI: percusión = canal 10 (índice 9); el resto en orden saltándolo (`midi_channels`).
- Atajos y teclas: ver `app/README.md`.

## 4. Verificación hecha (resultados)
- `cargo test` en `crates/nova-core`: **70 tests OK** (62 + 8 de `quantize`). `npx tsc --noEmit`: limpio.
- Contra MuseScore 4.6 real (CLI `MuseScore4.exe -o …`): las 42 plantillas incluidas en MuseScore, *Canon in D* y *Nocturne Op. 9 n.º 2* (archivos del usuario, **no incluidos en el repo**), y las 11 muestras de `samples/` salen con notas, spanners y marcas **idénticos** tras pasar por nuestro escritor y por MuseScore. *Big Score* (26 instrumentos, 22 k notas) coincide salvo la transposición de instrumentos (artefacto de la comparación).
- Diferencia conocida: dos pedales solapados al final del Chopin los interpreta distinto MuseScore.
- Interfaz: se revisó el **dibujo** exportando PDF y rasterizándolo. **Los botones y el teclado de la UI no se han probado a mano** en ninguna sesión (solo compilación y exportación automática) — probar a fondo con `npm run tauri dev`.

## 5. Puesta en marcha en el equipo nuevo
Requisitos: Rust estable (≥1.98 usado), Node 20+, **Visual Studio Build Tools (C++)** y **WebView2** en Windows (en Linux/macOS Tauri 2 también corre, pero no se ha probado).
```
# 1. descomprimir NovaScore-transfer.zip (no incluye target/ node_modules/ dist/)
cd NovaScore/app
npm install
cd crates/nova-core && cargo test                 # primera compilación: varios minutos
cd ../.. && npm run tauri dev                      # la primera vez compila src-tauri (~5-10 min)
```
- Archivos grandes dentro del repo: `app/public/soundfonts/MuseScore_General.sf3` (40 MB, MIT; se puede volver a bajar de ftp.osuosl.org/pub/musescore/soundfont/MuseScore_General/) y `app/public/fonts/Bravura.*` (SIL OFL).
- Opcional (verificación de formatos): instalar MuseScore 4 y, si no está en la ruta por defecto, definir `MUSESCORE` con la ruta del ejecutable.
- `scripts/pdf-to-png.ps1` (Windows 10/11): `-pdf f.pdf -out f.png -width 1800 -pg 0` rasteriza una página para ver el dibujo sin abrir la app.
- Consejo: poner `CARGO_TARGET_DIR` fuera del proyecto (el directorio `target` de Tauri llega a ~11 GB).
- **Memoria de Claude**: abre Claude Code en la carpeta del proyecto; `CLAUDE.md` se carga solo. Para restaurar además la memoria personal, copia `docs/claude-memory/*.md` a `~/.claude/projects/<carpeta-del-proyecto-con-guiones>/memory/` (ej. `C--Users-xxx-NovaScore`); si no existe la carpeta, créala. Nada de esto es imprescindible: `CLAUDE.md` + este documento bastan.

## 6. Convenciones de trabajo
Ver `CLAUDE.md` (idioma español, orden por módulos, informar con honestidad lo no probado, preguntar antes de cambios de rumbo, GPL-3 y componentes libres).

## 7. Siguiente bloque: IA local (plan propuesto, sin empezar)
Objetivo del plan original: transcribir audio a partitura, reconocer partituras desde PDF/imagen (OMR), detectar notas y acordes, asistir a la composición — todo local, gratuito y de código abierto.
**Antes de integrar cualquier modelo: comprobar la licencia de código *y de pesos* (algunos pesos son CC-BY-NC).** Lo siguiente son candidatos, no verificados en esta sesión:

| Tarea | Candidato | Notas |
|---|---|---|
| Audio → notas (polifónico, cualquier instrumento) | **Basic Pitch** (Spotify, Apache-2.0) | Ligero, corre bien en CPU; hay modelo ONNX/TF; salida notas+MIDI. Mejor primer paso |
| Audio → notas (piano, alta precisión) | ByteDance *piano transcription* | Mejor con GPU; revisar licencia de pesos |
| Audio multi-instrumento | MT3 (Google) | Pesado (JAX/T5X), pide GPU |
| Separar pistas antes de transcribir | Demucs (MIT) | Mejora mucho la mezcla; GPU recomendada |
| Tempo/compás/pulsos | librosa (ISC), *beat_this* | Para cuantizar sobre el pulso en vez de una rejilla fija |
| OMR (PDF/imagen → MusicXML) | **Audiveris** (AGPL-3, Java) como proceso externo; alternativas *oemer*, *homr* (verificar licencias) | Sale MusicXML → ya tenemos `import_musicxml` |
| Acordes | plantillas sobre cromagrama, o modelos abiertos | Los pesos de madmom son no comerciales: evitar |
| Asistente de composición | Magenta (Apache-2.0) y modelos simbólicos pequeños | Evitar pesos CC-BY-NC (p. ej. MusicGen) |

### Arquitectura sugerida
- **Proceso auxiliar ("sidecar") + gestor de modelos**: la app (Tauri) lanza un ejecutable local que recibe un archivo y devuelve **MIDI o MusicXML**; la app lo importa con los importadores que ya existen (`import_midi`, `import_musicxml`) y muestra un borrador editable con avisos de calidad. Así la IA queda desacoplada y es opcional.
- Modelos descargables bajo demanda a la carpeta de datos de la app (no empaquetados) para mantener el instalador ligero; mostrar tamaño y licencia antes de descargar.
- Inferencia con **ONNX Runtime** (crate `ort`, MIT/Apache-2.0) que admite CUDA/DirectML → mismo código en CPU y GPU. Python solo para prototipar/convertir modelos.
- Mejora necesaria en el núcleo para audio→partitura: **cuantización musical** (hoy el MIDI se cuantiza a semicorcheas fijas): detectar tempo/compás, tresillos, voces y separar manos. El punto de entrada natural es `io.rs::import_midi` + `Score::put_abs`.
- Calidad: probar con grabaciones monofónicas simples → piano → polifonía; medir con partituras conocidas (comparar notas con MusicXML de referencia).

### Progreso (2026-11-04)
- Decisión: modelos grandes **bajo demanda**; Basic Pitch (225 KB) va incluido en el binario.
- Hecho: `crates/nova-audio` (decodificación symphonia, remuestreo, inferencia `ort`, port Rust del extractor de notas; coincide con Basic Pitch Python en las 17 notas de prueba) y `nova-core/src/quantize.rs` (tempo por concentración de inicios con prior ~100 bpm, tonalidad Krumhansl, corte en do central, hasta 4 voces, duraciones simples). `Score::put_abs_voice` generaliza `put_abs`.
- Medido (ida y vuelta partitura→audio sintético→partitura, `examples/roundtrip`): recall ≈ 1,0; con `min_amplitude` 0,5 precisión 0,80–1,00 (F1 0,89–1,00). Solo audio sintético; **sin probar con grabaciones reales**.
- Límites: tempo constante, compás 4/4 supuesto, sin anacrusa, sin tresillos, sin dinámicas desde la amplitud, ambigüedad de octava de tempo (60 vs 120) resuelta por un prior (se puede forzar `tempo`).
- Hecho (2026-11-05): comando `import_audio` + botón «Importar audio» con diálogo (tempo automático/fijo, compás, sensibilidad = amplitud mínima).
- Seguimiento de pulso (2026-11-08, `nova-core/src/beats.rs`): mapa de tempos variable (programación dinámica tipo Ellis, búsqueda del periodo por candidatos con prior ~100 bpm, regla de preferir el doble de periodo si >120 bpm, acentos por acordes/graves/notas largas). `QuantizeOptions.follow_tempo` (por defecto sí) y `tightness`. Medido con `examples/roundtrip` (`NOVA_RUBATO=0.08 … -- @synthetic48 samples/long.musicxml`): rubato ±8 %: F1 de posición exacta 0,97 con seguimiento frente a 0,07–0,32 con tempo constante; **con ±15 % o más falla** (0,05–0,23): el periodo de partida y los pulsos saltados/duplicados desplazan el resto de la pieza. Sobre la grabación real del *Air* de Bach el tempo automático sale 62,5 (antes 124) pero la partitura sigue desordenada: ahora el cuello de botella son las notas falsas, las duraciones fragmentadas (silencios sueltos) y el reparto de voces.
- Limpieza de la salida (2026-11-08, `quantize.rs::clean_notes`): une fragmentos (misma altura, hueco ≤120 ms y la 2.ª sin inicio predicho por el modelo; `NoteEvent.onset`/`RawNote.onset`), quita armónicos (12/19/24/28/31 semitonos sobre una nota más fuerte que empieza a la vez, razón de amplitud ≤0,6) y notas cromáticas débiles/cortas fuera de la tonalidad. Umbral de amplitud **relativo** por defecto (`min_relative` 0,65 × percentil 90; da 0,42 en el *Air* real y ≈0,5 en las muestras sintéticas). Una voz por pentagrama por defecto (`max_voices`, 2 opcional en el diálogo) y relleno de huecos ≤360 ticks. Seguimiento de tempo solo con ≥24 notas. En el *Air*: silencios de 452 → 38 y partitura legible (compases completos, bajo en corcheas); siguen notas falsas sueltas y chords con alguna nota de más. Sintético sin rubato: F1 0,95 (igual que sin limpieza). Descubierto: las ~263 notas de igual altura «pegadas» en el *Air* son mayormente reataques reales, no fragmentos (solo 7 se unen).
- Otras grabaciones reales (2026-11-08, piano mp3): *Canon en re* (182 s, 29 s de cálculo): tonalidad correcta (re mayor) y bajo reconocible, pero solo el 40 % de las notas cae en la rejilla y 280 parecen tresillos (el intervalo más común entre notas es ⅓ de pulso): faltan **tresillos**. *Londonderry Air* (637 s, 43 s): la tonalidad sale dudosa (−2) y la limpieza quitó 135 notas cromáticas (arriesgado si la tonalidad falla); 48 % en rejilla, ritmo libre/rubato, acordes con notas de más; mucho peor que el *Air* de Bach. Conclusión: con grabaciones expresivas la salida es un borrador pobre; prioridades: tresillos, tonalidad más fiable (o no quitar cromáticas si la confianza es baja), tempo libre.
- Tresillos (2026-11-08): `Score::put_triplet_beat` (pulso de 3 ranuras de 160 ticks con notas/silencios, 1–2 ranuras por nota) y `Score::put_abs_voice_tied`; en `quantize.rs`, `detect_triplet_beats` (por fase de cada inicio dentro del pulso: más cerca del tercio que de la semicorchea; semilla = 2 inicios de tresillo, o 1 sin inicios rectos; se propaga a pulsos vecinos) y rejilla de tercios de negra en esos pulsos; las notas se parten en las barras de los pulsos de tresillos y se ligan. `QuantizeOptions.triplets` (por defecto sí). Medido con ±20 ticks de tolerancia (`NOVA_TOL=20`; con ±60 no se distingue porque un tresillo cuantizado a semicorchea solo se desvía 40): `@triplets32` F1 0,66 → 0,95 (0,94 con rubato ±8 %), `samples/tuplets.musicxml` 0,43 → 0,70 (contiene además quintillos y seisillos, **sin soportar**), resto de muestras sin cambio. En grabaciones: *Canon* 141 pulsos con tresillos y desaparece el aviso de rejilla (40 % → ≥60 %); *Londonderry* 59 % en rejilla (antes 48 %) con 207 pulsos de tresillos (posibles falsos positivos con tempo libre); *Air* de Bach 23 pulsos (sospechosos). Pendiente: quintillos/seisillos, y exigir más evidencia de tresillo cuando la confianza del pulso es baja. Visual: en el PDF se solapan algunas etiquetas «3» de grupos contiguos (layout existente).
- Más evidencia para tresillos y tonalidad dudosa (2026-11-08): `detect_triplet_beats` ahora usa una razón de verosimilitud por inicio, `(d16² − d3²)/2σ²`, con `σ` estimada de los datos (mediana de la distancia a la rejilla, entre 0,015 y 0,1 de pulso); se suma la evidencia del pulso con la mitad de la de sus vecinos y debe superar 2,5 (`TRIPLET_MIN_EVIDENCE`): con tempo inestable cada inicio pesa menos y hacen falta más pruebas. `estimate_key` devuelve también correlación y margen frente a la siguiente tonalidad de otro armado; si margen < 0,08 o correlación < 0,6 la tonalidad es **dudosa** y no se quitan notas cromáticas (aviso al usuario). Calibración con solo tres grabaciones: margen 0,33 *Canon*, 0,11 *Air*, 0,06 *Londonderry*; el 0,08 es una estimación entre ellas, hay que revisarlo con más datos. Efecto: sintético con tresillos sin cambio (F1 0,96/0,95); *Air* 23 → 10 pulsos de tresillos, *Londonderry* 207 → 79 (y 135 cromáticas ya no se eliminan; 53 % en rejilla), *Canon* 141 → 132; `tuplets.musicxml` 0,70 → 0,65 (24 notas, con quintillos/seisillos sin soporte).
- Quintillos y seisillos (2026-11-08): `Score::put_tuplet_beat` (3, 5 o 6 ranuras: 160/96/80 ticks; parte en ligadas lo que no tiene valor estándar, p. ej. 5 de 6 ranuras = 4 + 1) y `detect_tuplet_beats` (devuelve pulso → nº de ranuras). Verosimilitud por inicio `(d4² − dh²)/2σ² + ln(4/h)` (el segundo término es la parsimonia: sin él una rejilla densa explica cualquier cosa y salían cientos de seisillos falsos en grabaciones reales), penalización a priori (seisillos 1,5, quintillos 2,5 nats), evidencia propia mínima 1,5, evidencia total mínima 4,0 con la mitad de la de los vecinos, `σ` de los datos acotada a 0,015–0,045 y estimada solo con las rejillas de 16avos y tercios, y filtro de ranuras distintas ocupadas (≥2 tresillos, ≥4 quintillos/seisillos; tolerancia máx(2σ, 0,05)). Medido (tol ±20 ticks): a 60 negras/min `@quint32` F1 0,92 y `@sext32` 0,97; a 100 negras/min bajan (0,57/0,63) porque Basic Pitch descarta las notas de <127 ms (a 100 por minuto un quintillo dura 0,12 s): límite del modelo de audio, no del detector. Grabaciones: *Air* 5 tresillos, *Canon* 109 tresillos + pocos quintillos/seisillos, *Londonderry* aún con algunos (tempo libre).
- Cancelar/borrar el modelo de piano y `DirectML.dll` en el instalador: ver `docs/MODELOS.md` (2026-11-09). `src-tauri` tiene 5 tests (antes 3).
- Prueba manual del diálogo «Importar audio» con motor de piano y compás Automático (2026-11-09, `scripts/ui-test/`, exe de depuración): *Air* (Bach) con piano + Automático → 4/4 con anacrusa, Re M, tempo 62, dos manos, mensajes correctos; *Chopin Nocturno* con piano y 12/8 indicado → 12/8, Mi♭ M, tempo 93 (62 negras con puntillo/min), 47 s; *Canon* (268 s) con piano en 34 s (GPU) con la ventana respondiendo 16/16 comprobaciones, compás dudoso (5 %) → 4/4 con aviso. Modelo ausente (renombrado temporalmente): la opción «Piano» sale desactivada, aparece el aviso (98 MB, CC-BY-4.0) y el botón «Descargar modelo de piano»; la descarga muestra el progreso (7 → 88 %, ~8 s), el modelo descargado tiene el mismo SHA-256 que el anterior y la opción se activa sin reabrir el diálogo. Corregido: con compás indicado el aviso decía «detectado»; ahora dice «indicado». No probado: cancelar/borrar el modelo (no existen), sin conexión, otra GPU, reproducir el resultado, guardar/abrir `.nova` tras importar.
- Prueba del botón «Importar audio» en la interfaz real (2026-11-08, ver `scripts/ui-test/`): selector nativo → diálogo → «Transcribiendo audio…» → partitura cargada (título = nombre del archivo, tempo, compás, tonalidad, dos manos) → OK; segundo audio en la misma sesión con tempo 60, 3/4, 2 voces y sensibilidad 50 % → las opciones llegan (umbral 0,38 con 50 %, «Tempo indicado», compás 3/4) → OK; cancelar el selector → no cambia nada; archivo que no es audio → mensaje claro y la partitura anterior queda intacta; el *Canon* (182 s) por la interfaz: 31 s de cálculo con la ventana respondiendo (313 de ~314 latidos de prueba). Observación: deshacer tras importar no hace nada (la importación reemplaza el documento y reinicia el historial, igual que importar MIDI/MusicXML). No probado: reproducir el resultado, guardar/abrir `.nova` tras importar, el selector de voces en el resultado (solo en el test Rust).
- Tests de `src-tauri/src/audio.rs`: el JSON del diálogo (camelCase) se interpreta bien y `audio_to_score` respeta tempo/compás/voces sobre un WAV generado.
- Falta: probar el botón y el diálogo a mano (no se pudo: sin herramienta de escritorio ni extensión de Chrome conectada en esa sesión; se probó solo el lado Rust); probar con grabaciones reales; tresillos y detección de compás; aviso de descarga/licencia solo cuando se añadan modelos grandes.

### Requisitos de hardware orientativos
Basic Pitch: CPU, ~1–2 GB RAM. Demucs/MT3/piano de alta precisión: GPU NVIDIA con ≥6–8 GB VRAM recomendable. Audiveris: Java 17+, ~2 GB RAM.

## 8. Contenido del traspaso
`NovaScore-transfer.zip` (generado con `scripts/pack-project.ps1`; si Windows bloquea scripts, ejecutar su línea `tar -a -c -f …` a mano): código, `samples/`, fuentes y SoundFont, `CLAUDE.md`, `docs/` (este archivo + `claude-memory/`), `scripts/`. No incluye `target/`, `node_modules/`, `dist/`, `src-tauri/gen/` (se regeneran) ni las partituras personales del usuario.
