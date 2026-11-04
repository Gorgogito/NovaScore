---
name: novascore-decisions
description: "NovaScore: decisiones, estado y siguiente paso (IA local); detalle en docs/HANDOFF.md del proyecto"
metadata:
  type: project
---

NovaScore: editor de partituras libre (alternativa a MuseScore Studio). Código en `C:\JTorres\IA\NovaScore\app` (antes de pasarlo al equipo nuevo).

- Licencia GPL-3 aceptada. Stack: Tauri 2 + Rust (núcleo `crates/nova-core`) + TypeScript/Solid. Prioridad elegida: flujo de edición antes que compatibilidad; la compatibilidad MuseScore ya está hecha.
- Estado (2026-11-03): editor completo con 4 voces, tuplets, cambios de compás, repeticiones/casillas, ligaduras de expresión, reguladores, dinámicas, articulaciones, octavas, pedal, trinos, percusión, mezclador, SoundFont, PDF, MIDI, MusicXML y lectura/escritura `.mscz` (v2–v4), verificado contra MuseScore 4.6 (62 tests + round-trip con 42 plantillas y 11 muestras).
- **Siguiente bloque: IA local** (audio→partitura con Basic Pitch, OMR con Audiveris, acordes, asistente). Se está moviendo el proyecto a un equipo con más RAM y GPU para ello. Decisión abierta: modelos descargados bajo demanda (recomendado) o empaquetados.

**Why:** acordado con el usuario por pasos; el traspaso se preparó el 2026-11-03 con `CLAUDE.md`, `docs/HANDOFF.md` y `docs/claude-memory/` dentro del proyecto.
**How to apply:** en una sesión nueva leer `CLAUDE.md` y `docs/HANDOFF.md` (§7 = plan de IA). Verificar la licencia de *pesos* de modelos antes de integrarlos. Probar la UI a mano: los botones/teclado nunca se probaron salvo por compilación y exportación PDF.
