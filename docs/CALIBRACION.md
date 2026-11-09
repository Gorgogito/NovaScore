# Calibración de umbrales de la transcripción de audio

Herramientas (`app/crates/nova-audio/examples/`):
- `calibrate audio.mp3=partitura.mscz …` — alinea la transcripción con la partitura de referencia (DTW sobre cromogramas) y mide precisión/recall/F1 de notas (altura exacta, ±0,3 s) barriendo umbral relativo, margen de tonalidad, razón de armónicos y percentil cromático. Guarda la transcripción en `target/cache/` para repetir barridos sin volver a inferir.
- `tempo_sweep *.mp3` — tempo estimado según el umbral relativo, en varias grabaciones.
- `audio2score` acepta `NOVA_REL` y muestra notas/silencios del resultado.

## Datos (2026-11, carpeta `artefactos/`, no versionada)
11 grabaciones; 4 con partitura. **Solo Chopin sirve como referencia fiable** (calidad de alineación 0,79 y F 0,83 sin limpiar). Canon (0,69) y Air (0,55) son arreglos/versiones distintos de la grabación, y Londonderry dura 637 s frente a 72 compases: sus F absolutos no significan nada, solo se usan para comparar ajustes entre sí.

## Resultados
| Ajuste | Resultado |
|---|---|
| Umbral relativo | F (media 3 piezas): 0,3→0,48 · 0,5→0,48 · 0,65→0,44 · 0,8→0,34. En Chopin 0,65 pierde 0,06 de F (recall 0,83→0,66). **Nuevo valor por defecto: 0,5** (antes 0,65). Silencios resultantes: Air 47=47, Marx 162→147, Chopin 189→159. |
| Tempo vs umbral | Estable en 8 de 11 grabaciones. Inestable en Canon (67/133/92), Chopin (100→87 a partir de 0,6) y Londonderry. Con ≤0,5 Chopin es estable. Canon: la referencia da ≈134 (en unidades de la partitura): 0,5 acierta, 0,3–0,4 da la mitad (octava). |
| Armónicos (≤0,6) | Neutro en estas piezas (0,4 y 0,6 = sin quitar). Subirlo a 0,8+ quita notas reales y baja F. Se deja en 0,6. |
| Cromáticas / margen de tonalidad | Quitar cromáticas débiles baja F 0,449→0,443 (percentil 60) y →0,437 (95); solo afecta a Chopin y Air. La tonalidad se acierta en Canon (D) y Chopin (Eb) con margen 0,33–0,36; Air sale D frente a G de la partitura (arreglo distinto, margen 0,11). Sin evidencia para mover el margen 0,08: se deja. |

## Márgenes de tonalidad de las 11 grabaciones
0,36 Chopin, 0,36 Billy Joel, 0,33 Canon, 0,29 Marx, 0,27 Ofelia, 0,21 Autumn Leaves, 0,14 Love Story, 0,13 Monoman, 0,11 Air, 0,06 Londonderry, 0,06 Mariage d'Amour.

## Limitaciones
- Una sola referencia fiable: el barrido orienta, no demuestra. Hacen falta grabaciones con su propia partitura (misma versión) para calibrar de verdad; los tresillos/quintillos (penalizaciones) no se pueden calibrar sin ritmo de referencia.
- El tempo (octava y estabilidad) es el mayor riesgo de calidad, más que los umbrales de amplitud.
