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

## Motor de piano (ByteDance) frente a Basic Pitch — Chopin (única referencia fiable)
| | Basic Pitch | Piano |
|---|---|---|
| F1 sin umbral | 0,83 (P 0,83 / R 0,83) | **0,86** (P 0,86 / R 0,86) |
| Con umbral relativo 0,5 | 0,82 | 0,70 (recall 0,58) |
| Notas en el pipeline completo / silencios | 1291 / 159 | 1237 / 85 |

La velocidad del modelo de piano no es «ruido»: una velocidad baja es una nota suave real, así que con este motor **no se aplica umbral de amplitud** (el pipeline lo desactiva). La mejora medida es modesta porque la métrica (±0,3 s, DTW con calidad 0,70) es gruesa y la partitura de referencia no es exactamente la interpretación; no refleja el 0,97 publicado en MAESTRO. Canon y Air no sirven (arreglo distinto). Visualmente (PDF) la versión de piano tiene menos silencios espurios y acordes más limpios, pero el ritmo sigue limitado por el tempo/rubato (solo el 44 % de las notas cae en la rejilla).

## Detección de compás y anacrusa (`quantize::detect_meter`)
Método: sobre el mapa de pulsos se mide la fuerza de cada pulso (intensidad² de las notas que empiezan en él, más si son graves o largas, con poco peso por acorde, más un bonus cuando cambia la nota más grave) y se busca la hipótesis «tiempo fuerte cada m pulsos, con fase φ» (m = 2, 3 o 4; en 4/4 el tercer tiempo cuenta como secundario). La fase da la anacrusa. Sesgos a favor de 4/4 y de «sin anacrusa»; confianza = margen frente a la mejor hipótesis con otra fase (2/4 y 4/4 con la misma fase no compiten). Por debajo de 0,15 se usa 4/4 sin anacrusa y se avisa. El compás de 1 a 3 tiempos de anacrusa se escribe como silencios al inicio del primer compás (el modelo no tiene compás corto).

Evaluación (sin audio, con las notas exactas de 6 partituras en 4/4 y anacrusas simuladas de 0 a 3 tiempos, 24 casos): con confianza ≥ 0,15 las decisiones con anacrusa son correctas en 8 de 11 casos (fallan Canon en Re ×2 y Canon Rock ×1, todas con confianza baja); sin anacrusa, el valor por defecto acierta siempre que realmente no la hay. El compás salió 4/4 en las 6 (correcto en todas). Londonderry: 4/4 casos correctos. **Canon en Do y Canon en Re (ostinato de corcheas continuas) no tienen acentos que detectar**: confianza ≤ 0,15 → se queda con 4/4 sin anacrusa.

Grabaciones reales (11, sin verdad conocida): 10 salen 4/4, Monoman 3/4 (confianza 47 %), Air con anacrusa de 1 tiempo (36 %), el resto sin anacrusa; Chopin sin detección fiable. No hay forma de verificar los resultados reales sin su partitura exacta.

Límites: no se detectan compases compuestos (6/8, 9/8, 12/8: el pulso sigue siendo la negra y salen como 3/4 o 4/4 con tresillos), ni 5/4, 7/8 ni cambios de compás a mitad de pieza. Los sesgos y el umbral están calibrados con muy pocas piezas.

## Compases compuestos (6/8, 9/8, 12/8)
- **Modo compuesto** (`QuantizeOptions.compound` / compás 6/8, 9/8 o 12/8 indicado): el pulso seguido es la negra con puntillo (1440/2 = 720 ticks reales), dividido en tres corcheas y semicorcheas (rejilla de 1/6 de pulso; sin grupos irregulares). Internamente se cuantiza con 480 ticks por pulso y se pasa a ticks reales ×3/2. `Score.tempo` se guarda en negras por minuto (pulsos × 1,5). La anacrusa se cuenta en pulsos enteros.
- **Compás indicado sin tempo**: se sigue el pulso normal, y luego se vuelve a seguir con un periodo 1,5 veces mayor. Chopin op. 9 n.º 2 (12/8, piano) con «12/8»: 65 pulsos de negra con puntillo/min, sin el aviso de «rejilla de semicorcheas irregular» que daba antes (era 44 %), y en el PDF salen corcheas con puntillo y grupos coherentes.
- **Autodetección**: solo se activa si el seguimiento de pulso ya cae en la negra con puntillo y ≥ 50 % de los pulsos con notas se dividen en tres. Medido: en las 11 grabaciones reales el máximo con el pulso normal es 0,30, así que no hay falsos positivos; pero con las grabaciones sin tempo el seguimiento suele caer en la negra (Chopin: 0,07), así que **la autodetección casi nunca dispara** y para 6/8 y 12/8 hay que elegir el compás en el diálogo. Intenté detectarlo comparando con un pulso 1,5× más lento: no discrimina (acierto casual por hemiolia: piezas en 4/4 dan 0,7–1,0 de pulsos con tresillos), así que se descartó.
- **12/8 frente a 6/8**: en compuesto se prefiere 6/8 salvo evidencia jerárquica (el 1.er pulso más fuerte que el 3.º).
- **Cambio de criterio 2 vs 4 pulsos** (afecta también a los compases simples): 4/4 puntúa el contraste de los tiempos 1 y 3 contra 2 y 4 más un bonus si el 1.º es más fuerte que el 3.º; si son iguales es un patrón de dos tiempos y se prefiere la hipótesis más sencilla. Resultado en 24 casos simulados con confianza ≥ 0,15: 5 anacrusas correctas y 1 errónea (antes 8 y 3: más conservador); todos los compases salen 4/4. La novedad del bajo solo cuenta para notas graves (< 55).
- Pruebas: 3 tests (6/8 y 12/8, anacrusa de un pulso, compás fijo, y que un 4/4 de corcheas no se tome por compuesto), 88 en total.

## Estabilidad del tempo (`QuantizeOptions.robust_tempo`)
**Problema** (barrido del umbral relativo 0,3–0,8, motor general): Canon salía 67 / 133 / 68 / 92, Chopin 100 / 87 / 90 y Londonderry 98 / 85. La causa: el seguimiento de pulso usaba las notas que sobreviven al umbral del usuario, y el paisaje de puntuaciones de tempo es casi plano (Canon: picos a 92, 135 y 67 con puntuaciones 0,23 / 0,29 / 0,33 antes del prior; Chopin: cinco picos entre 77 y 120 con 0,19–0,21), así que un cambio mínimo en el conjunto de notas cambiaba el ganador. El 92 de Canon es una hemiolia (2/3 de 135), no una octava.

**Solución**: el pulso se sigue con un **conjunto fijo de notas** (las ≥ 0,5 × P90 de la amplitud de todas las detectadas, limpiadas), independiente del umbral del usuario, que solo decide qué notas se escriben. Es estable por construcción: en las 11 grabaciones el tempo es idéntico en los 7 umbrales (antes 3 inestables). Los valores coinciden con los que daba el umbral 0,5: Canon 133 (la pendiente del alineamiento con su partitura da ≈ 134 negras/min en la unidad de la partitura; 67 es su octava), Chopin 100 (la partitura da ≈ 49 negras/min: 100 es el nivel de la corchea), Londonderry 98. Con el motor de piano también es estable (Chopin 92, Canon 68, Air 62; sin él: 98→90→100, 68, 62→125). Test: `robust_tempo_does_not_depend_on_the_amplitude_threshold`.

**Probado y descartado**: elegir entre los 5 mejores picos usando la confianza de la detección de compás como evidencia extra (puntuación × (0,5 + confianza)). Estabilizaba el tempo pero movía grabaciones sin motivo verificable (Chopin 100→71, Love Story 100→66, Autumn Leaves 95→87), porque la confianza de compás tampoco discrimina bien entre niveles métricos. Y puntuar el apoyo armónico (puntuación a 2p, p/2): el paisaje es demasiado plano.

**Límites que quedan**: (1) con motores distintos el tempo puede diferir (Chopin 100 con Basic Pitch y 92 con piano: ambos coherentes con el rubato); (2) el nivel métrico (negra / corchea / blanca) sigue siendo ambiguo y se decide por el prior (~100 negras/min): para Canon, Chopin y Londonderry el usuario puede forzar el tempo en el diálogo; (3) la «verdad» de tempo solo se puede estimar con el alineamiento DTW, que es fiable para Chopin y apenas para Canon (Londonderry: inservible, la grabación son variaciones de 637 s frente a 72 compases).

## Banco con verdad conocida (`eval_scores`) — resuelve la falta de referencias
Problema anterior: solo Chopin servía como referencia (las demás partituras son otros arreglos que la grabación). Solución: **renderizar las propias partituras a audio** con el SoundFont de MuseScore (`oxisynth`, solo dev-dependency de los ejemplos: LGPL-2.1, no se distribuye; el SoundFont es MIT) y comparar la transcripción con la partitura original, que ahora es la verdad exacta (tempo, compás, tonalidad y cada nota en su tick). Sin rubato (tempo constante), 32 compases, 1 s de silencio inicial. 8 partituras: Londonderry, Canon Rock (guitarra eléctrica + cello + batería), Canon (fácil, completo y en Do), Chopin, Für Elise, Air. `cargo run --release --example eval_scores -- artefactos/partituras/*.mscz` (`NOVA_PIANO=modelo.onnx` para el motor de piano; `NOVA_HARM`, `NOVA_CHROMA`, `NOVA_MARGIN`, `NOVA_REL`, `NOVA_NOCLEAN`, `NOVA_NOTUPLETS`, `NOVA_TOL`…). Guarda las transcripciones en `target/evalset/` para experimentar sin repetir el audio (`key_eval`).

| Métrica | Basic Pitch | Piano |
|---|---|---|
| Tempo correcto (módulo octavas, ±4 %) | 8/8 | 8/8 |
| Compás | 6/8 | 6/8 (fallan Chopin 12/8→4/4 y Für Elise 3/8→3/4) |
| Tonalidad | **8/8** (antes 5/8) | **8/8** (antes 6/8) |
| F1 de notas, posición exacta ±60 ticks | 0,77 | **0,855** |
| F1 en las 5 piezas de piano real (sin Canon Rock) | 0,70–0,91 (Canon 0,53) | **0,94–1,00** |

- **El modelo de piano queda confirmado con datos exactos**: 0,97–1,00 en Canon fácil, Air, Canon en Do, Für Elise y Londonderry, frente al 0,53–0,94 de Basic Pitch. En Canon Rock (guitarra y batería) no sirve (0,03): es un modelo solo de piano.
- **Tonalidad**: los perfiles de Krumhansl fallaban en dominantes y menores (Londonderry A→E, Air G→D, Für Elise Am→E): 62 % (BP) y 75 % (piano) de acierto en las 8 piezas × 12 transposiciones. Los **perfiles de Temperley dan 96/96 (100 %) con ambos motores**; ahora son el valor por defecto (`KeyParams`). Los pesos extra del bajo o del final solo ayudaban a veces (final ×6: 100 % con piano, 75 % con BP). Con las 11 grabaciones reales ambos perfiles dan la misma tonalidad; los márgenes de Temperley son más uniformes (0,16–0,28), así que el umbral de «tonalidad dudosa» (0,08) deja de distinguir casos: queda como red de seguridad. Sin errores de tonalidad en el banco no se puede calibrar mejor.
- **Umbral relativo** (Basic Pitch): 0,3 → 0,736; 0,4 → 0,737; 0,5 → 0,749; 0,65 → 0,747. Se mantiene 0,5.
- **Limpieza** (Basic Pitch / piano, 20 compases): sin limpieza 0,799 / 0,852; por defecto 0,794 / 0,851; armónicos ≤0,4 0,798, ≤0,8 **0,758** (borra notas reales); cromático percentil 30 / 80: 0,796 / 0,793; margen de tonalidad 0 (siempre quitar) 0,792 / 0,849, 9 (nunca) 0,796 / 0,852. **La limpieza es neutra en audio limpio** (su razón de ser son las grabaciones reales ruidosas: silencios espurios, armónicos) y quitar cromáticas con un margen bajo no ayuda: se conservan los valores y no se endurecen.
- **Tresillos**: activar/desactivar no cambia el F1 (0,851 = 0,851): estas partituras no los tienen en los compases evaluados, así que **no hay falsos tresillos** en 8 piezas, pero tampoco se calibran las penalizaciones: faltan partituras con tresillos, quintillos o seisillos.
- **Compás**: los 4/4 salen bien; los compuestos (Chopin 12/8) y 3/8 no se detectan solos (ver sección de compases compuestos).

## Grupos irregulares (tresillos, quintillos, seisillos) en el banco
`scripts/gen_tuplet_scores.py` genera MusicXML con verdad exacta: `Tresillos.musicxml` (línea de violín, c. 1-8 de `Tresillos.pdf`, transcrita a mano) y 6 melodías sintéticas en 4/4 (tresillos 3:2, quintillos 5:4, seisillos 6:4 en el 30 % y el 50 % de los tiempos). `eval_scores` ahora lee `.musicxml`. Resultados (piano, F1 con tolerancia 60 ticks): tresillos 0,97-0,98, quintillos 0,95-0,97, seisillos 0,93-0,96; Tresillos.pdf 0,91 (precisión 0,83 por notas repetidas del modelo); activar/desactivar grupos no cambia F1 (tolerancia amplia), tempo y compás 7/7 y tonalidad 7/7. Con Basic Pitch los quintillos y seisillos bajan a 0,77-0,89 por **notas no detectadas** (recall 0,69-0,85, notas de 120 ms), no por el cuantizador.
- **Falso positivo hallado y corregido**: con un 50 % de tiempos en seisillos, la autodetección de compás compuesto saltaba (6/8, tempo ×1,5, F1 0,11) porque contaba los grupos de 6 como evidencia de compuesto. Ahora solo cuentan los de 3 (89 tests y banco completo sin cambios: 15 piezas, tempo 15/15, tonalidad 15/15, F1 medio 0,90).
- `Quintillos.pdf`, `Nocturna.pdf` y `Love Story.pdf` **no** se usaron como verdad: sin OMR habría que transcribirlos a ojo (ligaduras y puntillos densos en Quintillos; Nocturna es un tango de bandoneón de 2/4 muy denso; Love Story es un arreglo de violín distinto de la grabación de Indila).

## Lo que sigue sin calibrar
- Penalizaciones de tresillos/quintillos/seisillos: el banco sintético no las discrimina (F1 igual con y sin grupos); haría falta una métrica de ritmo más estricta (tolerancia < 30 ticks) o grupos mezclados con subdivisiones binarias cercanas.
- Rubato: el banco usa tempo constante. Se podría deformar el audio con una curva de tempo (`roundtrip` ya tiene `NOVA_RUBATO` con partituras sintéticas).
- Compases distintos de 4/4 y anacrusas reales: el banco apenas tiene (Für Elise 3/8, Chopin 12/8).
- Instrumentos que no son piano: solo Canon Rock (0,30 con Basic Pitch).
