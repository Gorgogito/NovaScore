# NovaScore

Editor de partituras libre (GPL-3.0-or-later). Tauri 2 + Rust + TypeScript (Solid).

- `crates/nova-core`: modelo de partitura, edición y deshacer/rehacer (Rust, con tests: `cargo test`).
- `src-tauri`: comandos Tauri (estado del documento, guardar/abrir `.nova`).
- `src`: interfaz (layout y render SMuFL con Bravura, reproducción provisional con Web Audio).

Desarrollo: `npm install` y `npm run tauri dev`.

Atajos: N entrada de notas · A–G notas · 0 silencio · 3–7 duración · . puntillo · +/- alteración ·
Mayús+A–G añade nota al acorde · T ligadura · ←/→ mover · ↑/↓ semitono (Ctrl: octava) · Supr borrar · Espacio reproducir · Ctrl+Z/Y · Ctrl+S/O/N.

Importar/exportar MusicXML y MIDI desde la barra superior. Ejemplo: `../samples/demo.musicxml` (también se puede pasar un .nova/.musicxml como argumento al ejecutable).

Exportar PDF (A4, vectorial, varias páginas) desde la barra superior; también `NOVASCORE_EXPORT_PDF=salida.pdf novascore.exe partitura.musicxml` exporta y cierra.

Repeticiones, casillas y trinos: fila «Repeticiones»: «|:» y «:|» marcan el inicio y el final de una repetición en el compás seleccionado; «1.» y «2.» crean las casillas sobre los compases seleccionados; «tr» pone un trino sobre la selección. La reproducción (y el MIDI) despliega las repeticiones y respeta las casillas, y los trinos alternan con la nota superior de la tonalidad. Ejemplo: `../samples/repeticiones.musicxml`.

Octavas y pedal: botones 8va / 8vb / 15ma / 15mb / Ped. de la fila «Líneas» sobre la selección. Bajo una octava las notas se escriben desplazadas pero suenan en su altura real (también al guardar en MuseScore, que guarda la altura escrita). El pedal suena (controlador 64) y va al MIDI. Ejemplo: `../samples/octavas_pedal.musicxml`.

Dinámicas y articulaciones: fila «Dinámicas / Articulaciones» bajo la barra. Una dinámica se aplica a la nota seleccionada (pulsar otra vez la quita); las articulaciones a todas las notas seleccionadas (Mayús+S staccato, Mayús+T tenuto, Mayús+M marcato). Afectan a la reproducción y al MIDI (velocidad, acortan o acentúan) y los reguladores interpolan la velocidad. Ejemplo: `../samples/dinamicas.musicxml`.

Ligaduras de expresión y reguladores: Mayús+flechas (o Mayús+clic) selecciona varias notas; S crea/quita una ligadura, < crescendo, > decrescendo; «Quitar expresión» borra los de la selección. Se leen y guardan en MuseScore (2/3/4) y MusicXML. Ejemplo: `../samples/ligaduras.musicxml`.

Guardar como MuseScore: «Exportar MuseScore» escribe .mscz/.mscx (formato 4.60). De un archivo MuseScore 4 se conservan, aunque no se muestren, dinámicas, articulaciones, letra, calderones, repeticiones, instrumentos y estilo; solo se pierden los saltos y marcas (D.C., segno, coda), igual que todo el marcado de archivos 2.x/3.x. Verificación: `cargo run --release --example ms_roundtrip -- entrada salida.mscz` abre el resultado con MuseScore y compara las notas.

Cambios de compás: elige el compás en el desplegable «Compás» tras seleccionar un compás; vale desde ahí hasta el siguiente cambio.

MuseScore: «Importar MuseScore» (o pasar el archivo como argumento) lee .mscz y .mscx de las versiones 2, 3 y 4. Herramienta de diagnóstico: `cargo run --release --example mscx_info -- archivo.mscz` (en `crates/nova-core`).

Tuplets: selecciona una figura y usa Ctrl+3 (tresillo), Ctrl+5, Ctrl+6, el desplegable «Tuplet…» de la barra o Ctrl+0 para deshacerlo; luego escribe las notas de cada hueco. Ejemplo: `../samples/tuplets.musicxml`.

Percusión: añade «Batería / percusión» en el panel; en modo entrada C=bombo, D=caja, E=charles, F=charles abierto, G=crash, A=ride, B=tom (o clic sobre el pentagrama). Ejemplo: `../samples/pendientes.musicxml`.

Voces: botones 1–4 de la barra o Ctrl+Alt+1–4 (hasta 4 voces por pentagrama; ejemplo `../samples/voices.musicxml`).

Panel de instrumentos (derecha): añadir/quitar, cambiar sonido GM, silenciar, solo y volumen. Ejemplos en `../samples/` (`duet.musicxml` tiene tres pentagramas).

Sonido: SpessaSynth (Apache-2.0) con el SoundFont MuseScore_General (MIT, ver `public/soundfonts/MuseScore_General_License.md`).
Fuente Bravura © Steinberg, SIL OFL 1.1.
