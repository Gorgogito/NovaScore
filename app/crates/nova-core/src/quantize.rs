//! Notas con tiempos en segundos (p. ej. de una transcripción de audio) → partitura.
//!
//! Pasos: filtrar por amplitud → estimar tempo y fase del pulso → estimar tonalidad →
//! cuantizar a semicorcheas → repartir en pentagramas (corte en do central) y en voces
//! (hasta 4 por pentagrama) → duraciones. Tempo constante: no sigue rubato.
//! No se generan tresillos; se avisa si parece haberlos.

use crate::beats::{find_beats, BeatMap};
use crate::io::ImportReport;
use crate::{Clef, Measure, Part, Pitch, Score, TimeSig, PPQ};
use std::collections::{BTreeMap, HashMap};

/// Rejilla de cuantización: semicorchea.
const GRID: u32 = PPQ / 4;
const MAX_MEASURES: u32 = 1000;
/// Do central: por debajo va al pentagrama de clave de fa.
const SPLIT_PITCH: u8 = 60;
/// Margen mínimo (frente a la siguiente tonalidad) y correlación mínima para fiarse de la tonalidad.
const KEY_MIN_MARGIN: f64 = 0.08;
const KEY_MIN_CORR: f64 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    /// 0..1
    pub amplitude: f32,
    /// La nota nace de un inicio claro (reataque). `false` = continuación / posible fragmento.
    pub onset: bool,
}

#[derive(Debug, Clone)]
pub struct QuantizeOptions {
    pub title: String,
    /// Descarta notas con menos amplitud (suele eliminar armónicos falsos).
    pub min_amplitude: f32,
    /// Si es `Some(r)`, el umbral es `r` × el percentil 90 de las amplitudes (se adapta a lo
    /// fuerte que suene la grabación) y `min_amplitude` se ignora.
    pub min_relative: Option<f32>,
    /// Tempo en negras por minuto; `None` = estimarlo.
    pub tempo: Option<f64>,
    pub time: TimeSig,
    /// Programa General MIDI del instrumento (0 = piano).
    pub program: u8,
    /// Huecos menores que esto (ticks) entre notas de una voz se rellenan alargando la nota.
    pub legato_ticks: u32,
    /// Seguir las variaciones de tempo (rubato) con un mapa de pulsos; `false` = tempo constante.
    pub follow_tempo: bool,
    /// Rigidez del seguimiento: mayor = el tempo varía menos.
    pub tightness: f64,
    /// Si es `Some(r)`, se calcula también el seguimiento con rigidez `r` (más laxa, para rubato) y se
    /// usa solo si los ataques caen claramente más cerca de la rejilla que con el rígido.
    pub lax_tightness: Option<f64>,
    /// Limpiar la transcripción (unir fragmentos, quitar armónicos y notas cromáticas débiles).
    pub clean: bool,
    /// Máximo de voces por pentagrama.
    pub max_voices: usize,
    /// Detectar pulsos de tresillos de corchea y escribirlos como tresillos.
    pub triplets: bool,
    /// Detectar el compás (2/4, 3/4 o 4/4) a partir de los acentos; sustituye a `time`.
    pub auto_meter: bool,
    /// Detectar la anacrusa (tiempos antes del primer compás completo).
    pub auto_pickup: bool,
    /// Reconocer compases compuestos (6/8, 9/8, 12/8) cuando casi todos los pulsos se dividen en tres.
    pub compound: bool,
    /// Seguir el pulso con un conjunto fijo de notas (las más fuertes de todas las detectadas), de modo
    /// que el tempo no dependa del umbral de amplitud elegido por el usuario.
    pub robust_tempo: bool,
    /// Ajustes de la limpieza previa (armónicos, notas cromáticas, margen de tonalidad).
    pub clean_params: CleanParams,
}

impl Default for QuantizeOptions {
    fn default() -> Self {
        QuantizeOptions {
            title: "Transcripción de audio".into(),
            min_amplitude: 0.5,
            min_relative: Some(0.5),
            tempo: None,
            time: TimeSig { num: 4, den: 4 },
            program: 0,
            legato_ticks: PPQ * 3 / 4,
            follow_tempo: true,
            tightness: 30.0,
            lax_tightness: None,
            clean: true,
            max_voices: 1,
            triplets: true,
            auto_meter: false,
            auto_pickup: false,
            compound: false,
            robust_tempo: false,
            clean_params: CleanParams::default(),
        }
    }
}

/// Resultado de la estimación de pulso.
#[derive(Debug, Clone, Copy)]
pub struct Beat {
    pub bpm: f64,
    /// Instante (s) de un pulso de la rejilla (en `[0, 60/bpm)`).
    pub phase: f64,
    /// 0..1: qué tan concentrados están los inicios en la rejilla.
    pub confidence: f64,
}

fn concentration(onsets: &[(f64, f32)], bpm: f64) -> (f64, f64) {
    let beat = 60.0 / bpm;
    let (mut re, mut im, mut tot) = (0.0f64, 0.0f64, 0.0f64);
    for &(t, w) in onsets {
        let a = std::f64::consts::TAU * t / beat;
        re += w as f64 * a.cos();
        im += w as f64 * a.sin();
        tot += w as f64;
    }
    if tot <= 0.0 {
        return (0.0, 0.0);
    }
    ((re * re + im * im).sqrt() / tot, im.atan2(re))
}

/// Tempo y fase del pulso a partir de los inicios (segundos, peso).
/// Busca el pulso que mejor agrupa los inicios y desempata con un prior hacia ~105 bpm.
pub fn estimate_beat(onsets: &[(f64, f32)]) -> Beat {
    let default = Beat { bpm: 100.0, phase: onsets.first().map_or(0.0, |o| o.0).rem_euclid(0.6), confidence: 0.0 };
    if onsets.len() < 3 {
        return default;
    }
    // Prior hacia ~100 bpm para desempatar octavas (60 vs 120, 72 vs 144).
    let prior = |bpm: f64| (-0.5 * ((bpm / 100.0).log2() / 0.5).powi(2)).exp();
    // Concentración a nivel de negra o, si solo hay corcheas (las negras se cancelan),
    // a nivel de corchea con una pequeña penalización. Devuelve (puntuación, confianza, fase en s).
    let grid = |bpm: f64| -> (f64, f64, f64) {
        let (c1, a1) = concentration(onsets, bpm);
        let (c2, a2) = concentration(onsets, bpm * 2.0);
        if c1 >= 0.6 * c2 {
            (c1, c1, a1.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU * 60.0 / bpm)
        } else {
            (0.6 * c2, c2, a2.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU * 30.0 / bpm)
        }
    };
    let (mut best, mut best_s) = (100.0, -1.0);
    let mut bpm = 40.0;
    while bpm <= 220.0 {
        let s = grid(bpm).0 * prior(bpm);
        if s > best_s {
            best_s = s;
            best = bpm;
        }
        bpm += 0.1;
    }
    // afinado (sin prior: ya se eligió la octava)
    let c0 = best;
    let mut bs = -1.0;
    let mut bpm = c0 - 0.3;
    while bpm <= c0 + 0.3 {
        let s = grid(bpm).0;
        if s > bs {
            bs = s;
            best = bpm;
        }
        bpm += 0.01;
    }
    let (_, conf, phase) = grid(best);
    Beat { bpm: best, phase, confidence: conf }
}

/// Tonalidad (número de alteraciones, + sostenidos / − bemoles) por perfiles de Krumhansl.
pub fn estimate_fifths(notes: &[RawNote]) -> i8 {
    estimate_key(notes).0
}

/// Tonalidad y su fiabilidad: `(alteraciones, correlación del mejor candidato, margen)` donde el
/// margen es la diferencia con la mejor tonalidad de otro armado (la relativa mayor/menor cuenta
/// como la misma). Un margen pequeño o una correlación baja indican una tonalidad dudosa.
pub fn estimate_key(notes: &[RawNote]) -> (i8, f64, f64) {
    estimate_key_with(notes, &KeyParams::default())
}

/// Ajustes de la estimación de tonalidad.
#[derive(Debug, Clone, Copy)]
pub struct KeyParams {
    /// Peso de las notas graves (< E3): la tónica y la dominante suelen estar en el bajo.
    pub bass_gain: f64,
    /// Peso de las notas del último 8 % de la pieza (el final suele ser la tónica).
    pub end_gain: f64,
    /// Perfiles de Temperley (por defecto: 96/96 en el banco `eval_scores`) en vez de los de Krumhansl (60-72/96).
    pub temperley: bool,
}

impl Default for KeyParams {
    fn default() -> Self {
        KeyParams { bass_gain: 1.0, end_gain: 1.0, temperley: true }
    }
}

pub fn estimate_key_with(notes: &[RawNote], p: &KeyParams) -> (i8, f64, f64) {
    let mut major: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    let mut minor: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    if p.temperley {
        major = [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0];
        minor = [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0];
    }
    // alteraciones de la tonalidad mayor cuya tónica es la clase de altura i
    const MAJOR_FIFTHS: [i8; 12] = [0, -5, 2, -3, 4, -1, 6, 1, -4, 3, -2, 5];
    let end = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    let begin = notes.iter().map(|n| n.start).fold(f64::MAX, f64::min);
    let tail_from = end - 0.08 * (end - begin).max(0.0);
    let mut hist = [0.0f64; 12];
    for n in notes {
        let mut w = (n.end - n.start).max(0.05) * n.amplitude as f64;
        if n.pitch < 53 {
            w *= p.bass_gain;
        }
        if n.start >= tail_from {
            w *= p.end_gain;
        }
        hist[(n.pitch % 12) as usize] += w;
    }
    if hist.iter().sum::<f64>() <= 0.0 {
        return (0, 0.0, 0.0);
    }
    let corr = |profile: &[f64; 12], tonic: usize| -> f64 {
        let x: Vec<f64> = (0..12).map(|i| hist[(i + tonic) % 12]).collect();
        let (mx, my) = (x.iter().sum::<f64>() / 12.0, profile.iter().sum::<f64>() / 12.0);
        let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            sxy += (x[i] - mx) * (profile[i] - my);
            sxx += (x[i] - mx).powi(2);
            syy += (profile[i] - my).powi(2);
        }
        if sxx <= 0.0 || syy <= 0.0 { 0.0 } else { sxy / (sxx * syy).sqrt() }
    };
    // mejor correlación por armado (mayor o menor relativa)
    let mut by_fifths: std::collections::BTreeMap<i8, f64> = std::collections::BTreeMap::new();
    for tonic in 0..12 {
        for (c, f) in [(corr(&major, tonic), MAJOR_FIFTHS[tonic]), (corr(&minor, tonic), MAJOR_FIFTHS[(tonic + 3) % 12])] {
            let e = by_fifths.entry(f).or_insert(f64::MIN);
            if c > *e {
                *e = c;
            }
        }
    }
    let mut ranked: Vec<(i8, f64)> = by_fifths.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let margin = ranked.get(1).map_or(1.0, |s| ranked[0].1 - s.1);
    (ranked[0].0, ranked[0].1, margin)
}

#[derive(Clone, Copy)]
struct Q {
    s: u32,
    e: u32,
    pitch: u8,
}

struct Group {
    s: u32,
    e: u32,
    pitches: Vec<u8>,
}


/// Evidencia mínima (en nats) para marcar un pulso como de grupo irregular.
/// Evidencia propia mínima del pulso (sin contar a los vecinos), para que un pulso recto junto
/// a una racha de grupos no se contagie.
const TUPLET_MIN_OWN: f64 = 1.5;
const TUPLET_MIN_EVIDENCE: f64 = 4.0;
/// Grupos irregulares por pulso y su penalización a priori en nats (los quintillos y los
/// seisillos son mucho menos frecuentes que los tresillos).
/// Ranuras distintas mínimas ocupadas por inicios para aceptar cada tipo (un grupo real de
/// cinco o seis notas deja varias): los grupos densos explican casi cualquier inicio suelto.
const TUPLET_KINDS_MIN_SLOTS: [u32; 3] = [2, 4, 4];
const TUPLET_KINDS: [(u32, f64); 3] = [(3, 0.0), (6, 1.5), (5, 2.5)];

/// Pulsos (índices de negra) cuyos inicios encajan mejor en un grupo irregular (3, 5 o 6 por
/// pulso) que en semicorcheas, con el número de ranuras del grupo. `ticks` son las posiciones
/// sin cuantizar, en ticks desde el primer pulso.
///
/// Cada inicio aporta `(d4² − dh²) / 2σ² + ln(4/h)`: la razón de verosimilitud entre «cae en la
/// rejilla de `h` ranuras por pulso» y «cae en una semicorchea» con ruido gaussiano de desviación
/// `σ` (el segundo término es la parsimonia: una rejilla más densa explica más inicios). `σ` se
/// estima de los propios datos (mediana de la distancia de cada inicio a la rejilla más cercana),
/// de modo que con un tempo inestable cada inicio pesa menos y hacen falta más pruebas. Se suma
/// la evidencia del pulso con la mitad de la de sus vecinos, se resta la penalización a priori del
/// tipo de grupo y debe superar `TUPLET_MIN_EVIDENCE`; se elige el tipo con más evidencia.
pub fn detect_tuplet_beats(ticks: &[f64]) -> HashMap<u32, u8> {
    let phase = |t: f64| {
        let beat = (t / PPQ as f64).floor() as u32;
        (beat, t / PPQ as f64 - beat as f64)
    };
    let dist = |phi: f64, h: u32| (phi * h as f64 - (phi * h as f64).round()).abs() / h as f64;
    let mut nearest: Vec<f64> = ticks
        .iter()
        .filter(|&&t| t >= 0.0)
        .map(|&t| {
            let phi = phase(t).1;
            [4, 3].iter().map(|&h| dist(phi, h)).fold(f64::MAX, f64::min)
        })
        .collect();
    if nearest.len() < 2 {
        return HashMap::new();
    }
    nearest.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let sigma = (1.4826 * nearest[nearest.len() / 2]).clamp(0.015, 0.045);
    // evidencia por pulso y por tipo de grupo
    let mut score: BTreeMap<u32, [f64; 3]> = BTreeMap::new();
    // ranuras distintas de cada tipo de grupo que ocupan los inicios del pulso (máscara de bits)
    let mut slots: BTreeMap<u32, [u32; 3]> = BTreeMap::new();
    for &t in ticks.iter().filter(|&&t| t >= 0.0) {
        let (beat, phi) = phase(t);
        let d4 = dist(phi, 4);
        let e = score.entry(beat).or_insert([0.0; 3]);
        let m = slots.entry(beat).or_insert([0; 3]);
        for (k, &(h, _)) in TUPLET_KINDS.iter().enumerate() {
            let dh = dist(phi, h);
            if dh < (2.0 * sigma).max(0.05) {
                m[k] |= 1 << (((phi * h as f64).round() as u32) % h);
            }
            // verosimilitud con ruido gaussiano + masa de probabilidad por punto de la rejilla
            // (una rejilla más densa explica cualquier inicio y debe pagarlo: ln(4/h) por inicio)
            e[k] += (d4 * d4 - dh * dh) / (2.0 * sigma * sigma) + (4.0 / h as f64).ln();
        }
    }
    let mut out = HashMap::new();
    for (&b, s) in &score {
        let near = |b2: u32, k: usize| score.get(&b2).map_or(0.0, |v| v[k]);
        let mut best: Option<(f64, u8)> = None;
        for (k, &(h, penalty)) in TUPLET_KINDS.iter().enumerate() {
            let window = s[k] + 0.5 * (near(b.wrapping_add(1), k) + if b > 0 { near(b - 1, k) } else { 0.0 });
            let value = window - penalty;
            let distinct = slots.get(&b).map_or(0, |m| m[k].count_ones());
            if distinct >= TUPLET_KINDS_MIN_SLOTS[k]
                && s[k] > TUPLET_MIN_OWN
                && value > TUPLET_MIN_EVIDENCE
                && best.map_or(true, |(v, _)| value > v)
            {
                best = Some((value, h as u8));
            }
        }
        if let Some((_, h)) = best {
            out.insert(b, h);
        }
    }
    out
}

/// Ajusta una posición (ticks) a la rejilla de su pulso: en un pulso con grupo irregular, a
/// 1/3, 1/5 o 1/6 de negra; en los demás, a semicorcheas.
fn snap(t: f64, tuplets: &HashMap<u32, u8>) -> u32 {
    let t = t.max(0.0);
    let beat = (t / PPQ as f64).floor() as u32;
    match tuplets.get(&beat) {
        Some(&n) => {
            let slot = PPQ as f64 / n as f64;
            beat * PPQ + (((t - (beat * PPQ) as f64) / slot).round() * slot).round() as u32
        }
        None => (t / GRID as f64).round() as u32 * GRID,
    }
}

/// Ticks de una ranura en el pulso `beat` (una semicorchea si no hay grupo irregular).
fn slot_ticks(beat: u32, tuplets: &HashMap<u32, u8>) -> u32 {
    tuplets.get(&beat).map_or(GRID, |&n| PPQ / n as u32)
}

/// ¿Toca el intervalo `[s, e)` algún pulso con grupo irregular?
fn touches_tuplet(s: u32, e: u32, tuplets: &HashMap<u32, u8>) -> bool {
    !tuplets.is_empty() && (s / PPQ..=e.saturating_sub(1).max(s) / PPQ).any(|b| tuplets.contains_key(&b))
}

/// Reparte notas ya cuantizadas de un pentagrama en hasta 4 voces (la 0 = la más aguda).
fn allocate_voices(mut notes: Vec<Q>, max_voices: usize, legato: u32, tuplets: &HashMap<u32, u8>) -> Vec<Vec<Group>> {
    notes.sort_by(|a, b| a.s.cmp(&b.s).then(b.pitch.cmp(&a.pitch)));
    let mut voices: Vec<Vec<Group>> = Vec::new();
    let mean = |g: &Group| g.pitches.iter().map(|&p| p as f64).sum::<f64>() / g.pitches.len() as f64;
    for n in notes {
        // 1. acorde con un grupo que empieza igual y dura casi lo mismo
        if let Some(g) = voices
            .iter_mut()
            .filter_map(|v| v.last_mut())
            .find(|g| g.s == n.s && g.e.abs_diff(n.e) <= GRID)
        {
            if !g.pitches.contains(&n.pitch) {
                g.pitches.push(n.pitch);
            }
            g.e = g.e.min(n.e).max(g.s + GRID);
            continue;
        }
        // 2. voz libre más cercana en altura
        let free = voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.last().map_or(true, |g| g.e <= n.s))
            .min_by(|a, b| {
                let da = a.1.last().map_or(0.0, |g| (mean(g) - n.pitch as f64).abs());
                let db = b.1.last().map_or(0.0, |g| (mean(g) - n.pitch as f64).abs());
                da.partial_cmp(&db).unwrap()
            })
            .map(|(i, _)| i);
        let target = match free {
            Some(i) => i,
            None if voices.len() < max_voices => {
                voices.push(Vec::new());
                voices.len() - 1
            }
            None => {
                // todas ocupadas: cortar la voz que acaba antes
                let i = voices.iter().enumerate().min_by_key(|(_, v)| v.last().map_or(0, |g| g.e)).map(|(i, _)| i).unwrap();
                if let Some(g) = voices[i].last_mut() {
                    if g.s == n.s {
                        if !g.pitches.contains(&n.pitch) {
                            g.pitches.push(n.pitch);
                        }
                        continue;
                    }
                    g.e = n.s;
                }
                i
            }
        };
        voices[target].push(Group { s: n.s, e: n.e, pitches: vec![n.pitch] });
    }
    // legato: cerrar huecos pequeños; sin solapes
    for v in &mut voices {
        for i in 0..v.len().saturating_sub(1) {
            let next = v[i + 1].s;
            if v[i].e > next || next - v[i].e < legato {
                v[i].e = next;
            }
        }
    }
    // Si tras la nota hay silencio, una duración que exigiría ligadura (p. ej. 600 ticks)
    // se ajusta a la figura simple más cercana: el modelo suele alargar los finales.
    const SIMPLE: [u32; 8] = [120, 240, 360, 480, 720, 960, 1440, 1920];
    for v in &mut voices {
        for i in 0..v.len() {
            let next = v.get(i + 1).map(|g| g.s);
            let d = v[i].e - v[i].s;
            if next != Some(v[i].e) && d <= 1920 && !SIMPLE.contains(&d) && !touches_tuplet(v[i].s, v[i].e, tuplets) {
                let best = SIMPLE.iter().copied().min_by_key(|&s| (s.abs_diff(d), s)).unwrap();
                v[i].e = v[i].s + best;
            }
        }
    }
    voices.retain(|v| !v.is_empty());
    voices.sort_by(|a, b| {
        let m = |v: &Vec<Group>| v.iter().map(mean).sum::<f64>() / v.len() as f64;
        m(b).partial_cmp(&m(a)).unwrap()
    });
    voices
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CleanStats {
    pub merged: usize,
    pub harmonics: usize,
    pub chromatic: usize,
    /// La tonalidad estimada no es fiable: no se quitaron notas cromáticas.
    pub key_doubtful: bool,
}

/// Une las notas de la misma altura separadas por un hueco mínimo (el modelo parte las notas
/// largas en fragmentos contiguos).
fn merge_fragments(notes: &[RawNote], max_gap: f64) -> (Vec<RawNote>, usize) {
    let mut v: Vec<RawNote> = notes.to_vec();
    v.sort_by(|a, b| (a.pitch, a.start.to_bits()).cmp(&(b.pitch, b.start.to_bits())));
    let mut out: Vec<RawNote> = Vec::with_capacity(v.len());
    let mut merged = 0;
    for n in v {
        match out.last_mut() {
            Some(last) if last.pitch == n.pitch && !n.onset && n.start - last.end <= max_gap => {
                let (dl, dn) = (last.end - last.start, n.end - n.start);
                last.amplitude = (last.amplitude * dl as f32 + n.amplitude * dn as f32) / (dl + dn) as f32;
                last.end = last.end.max(n.end);
                merged += 1;
            }
            _ => out.push(n),
        }
    }
    (out, merged)
}

/// Quita notas que son armónicos de otra más fuerte: 12, 19, 24, 28 o 31 semitonos más
/// agudas, que empiezan a la vez y con amplitud claramente menor.
fn remove_harmonics(notes: &[RawNote], max_ratio: f32) -> (Vec<RawNote>, usize) {
    let mut sorted: Vec<RawNote> = notes.to_vec();
    sorted.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    let mut keep = vec![true; sorted.len()];
    for i in 0..sorted.len() {
        let n = sorted[i];
        let mut j = i;
        while j > 0 && n.start - sorted[j - 1].start < 0.08 {
            j -= 1;
        }
        let ghost = (j..sorted.len()).take_while(|&k| sorted[k].start - n.start < 0.08).any(|k| {
            let l = &sorted[k];
            n.pitch > l.pitch && [12, 19, 24, 28, 31].contains(&(n.pitch - l.pitch)) && n.amplitude <= max_ratio * l.amplitude
        });
        keep[i] = !ghost;
    }
    let out: Vec<RawNote> = sorted.iter().zip(&keep).filter(|(_, k)| **k).map(|(n, _)| *n).collect();
    let removed = sorted.len() - out.len();
    (out, removed)
}

/// Quita notas cortas y débiles cuya clase de altura no está en la tonalidad (ni en las
/// alteraciones habituales del modo menor).
fn remove_chromatic(notes: &[RawNote], fifths: i8, pct: usize) -> (Vec<RawNote>, usize) {
    let tonic = ((fifths as i32 * 7).rem_euclid(12)) as u8;
    let mut allowed = [false; 12];
    for s in [0, 2, 4, 5, 7, 9, 11] {
        allowed[((tonic + s) % 12) as usize] = true;
    }
    let minor = (tonic + 9) % 12;
    allowed[((minor + 11) % 12) as usize] = true; // sensible
    allowed[((minor + 6) % 12) as usize] = true; // sexto grado melódico
    let mut amps: Vec<f32> = notes.iter().map(|n| n.amplitude).collect();
    amps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let weak = amps.get(amps.len() * pct / 100).copied().unwrap_or(0.0);
    let out: Vec<RawNote> = notes
        .iter()
        .copied()
        .filter(|n| allowed[(n.pitch % 12) as usize] || n.amplitude >= weak || n.end - n.start >= 0.5)
        .collect();
    let removed = notes.len() - out.len();
    (out, removed)
}

/// Parámetros de la limpieza (los valores por defecto son los calibrados; ver `docs/CALIBRACION.md`).
#[derive(Debug, Clone, Copy)]
pub struct CleanParams {
    /// Margen de tonalidad por debajo del cual se considera dudosa.
    pub key_min_margin: f64,
    /// Correlación de tonalidad por debajo de la cual se considera dudosa.
    pub key_min_corr: f64,
    /// Razón de amplitud máxima armónico/fundamental para borrar un armónico.
    pub harmonic_ratio: f32,
    /// Percentil (0-100) de amplitud bajo el cual una nota cromática débil se borra.
    pub chromatic_pct: usize,
}

impl Default for CleanParams {
    fn default() -> Self {
        CleanParams { key_min_margin: KEY_MIN_MARGIN, key_min_corr: KEY_MIN_CORR, harmonic_ratio: 0.6, chromatic_pct: 60 }
    }
}

/// Limpieza previa a la cuantización. Devuelve las notas y cuántas se unieron / quitaron.
pub fn clean_notes(notes: &[RawNote]) -> (Vec<RawNote>, CleanStats) {
    clean_notes_with(notes, &CleanParams::default())
}

pub fn clean_notes_with(notes: &[RawNote], p: &CleanParams) -> (Vec<RawNote>, CleanStats) {
    let (a, merged) = merge_fragments(notes, 0.12);
    let (b, harmonics) = remove_harmonics(&a, p.harmonic_ratio);
    let (fifths, corr, margin) = estimate_key(&b);
    let key_doubtful = margin < p.key_min_margin || corr < p.key_min_corr;
    let (c, chromatic) = if key_doubtful { (b, 0) } else { remove_chromatic(&b, fifths, p.chromatic_pct) };
    (c, CleanStats { merged, harmonics, chromatic, key_doubtful })
}

/// Compás y anacrusa estimados.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterGuess {
    /// Negras por compás.
    pub beats: u32,
    /// Tiempos que preceden al primer tiempo fuerte (0 = la pieza empieza en el primer tiempo).
    pub pickup: u32,
    /// 0..1: cuánto destaca la mejor hipótesis sobre la siguiente.
    pub confidence: f64,
}

/// Fuerza de cada pulso (relativa al primer pulso con notas): acentos de las notas que empiezan
/// en él más un bonus cuando cambia la nota más grave (cambio de armonía, típico del tiempo fuerte).
fn beat_strengths(notes: &[RawNote], map: &BeatMap, k0: i64) -> Vec<f64> {
    let last = notes.iter().map(|n| map.pos(n.start)).fold(0.0, f64::max);
    let n_beats = ((last - k0 as f64).floor() as i64 + 1).max(1) as usize;
    let mut acc = vec![0.0f64; n_beats];
    let mut peak = vec![0.0f64; n_beats];
    let mut bass: Vec<Option<u8>> = vec![None; n_beats];
    for n in notes {
        // fuerza de la nota: intensidad, más si es grave o larga (no se cuenta cuántas suenan a la vez:
        // los acordes de los tiempos débiles no deben ganar al bajo del tiempo fuerte)
        let mut wt = (n.amplitude as f64).powi(2);
        let together = notes.iter().filter(|m| (m.start - n.start).abs() < 0.04).count() as f64;
        wt *= 1.0 + METER_CHORD_GAIN * (together - 1.0).min(3.0);
        if n.pitch < 55 {
            wt *= 1.3;
        }
        if n.end - n.start > 0.7 {
            wt *= 1.3;
        }
        let p = map.pos(n.start);
        let k = p.round();
        if (p - k).abs() > 0.3 {
            continue;
        }
        let j = k as i64 - k0;
        if j < 0 || j as usize >= n_beats {
            continue;
        }
        let j = j as usize;
        acc[j] += 0.25 * wt;
        peak[j] = peak[j].max(wt);
        if n.pitch < 55 && bass[j].map_or(true, |b| n.pitch < b) {
            bass[j] = Some(n.pitch);
        }
    }
    for j in 0..n_beats {
        acc[j] += peak[j];
    }
    let mean = acc.iter().sum::<f64>() / acc.iter().filter(|&&a| a > 0.0).count().max(1) as f64;
    let mut prev: Option<u8> = None;
    let mut s = vec![0.0; n_beats];
    for j in 0..n_beats {
        s[j] = acc[j] / mean.max(1e-9);
        if let Some(b) = bass[j] {
            if prev.map_or(false, |p| p % 12 != b % 12) {
                s[j] += 0.5;
            }
            prev = Some(b);
        }
    }
    s
}

/// Puntuación de «los tiempos fuertes caen en `phase` módulo `m`» (m = 2, 3 o 4).
/// Para m = 4 se mide el contraste de los tiempos 1 y 3 frente a 2 y 4, y se añade el bonus
/// jerárquico: el primer tiempo más fuerte que el tercero (si son iguales es un patrón de dos tiempos).
fn downbeat_score(s: &[f64], m: usize, phase: usize) -> f64 {
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    let step = if m == 4 { 2 } else { m };
    let ph = if m == 4 { phase % 2 } else { phase };
    let (mut ds, mut dn, mut os, mut on) = (0.0, 0usize, 0.0, 0usize);
    for (j, &v) in s.iter().enumerate() {
        if j % step == ph {
            ds += v;
            dn += 1;
        } else {
            os += v;
            on += 1;
        }
    }
    if dn == 0 || on == 0 {
        return f64::MIN;
    }
    let mut sc = (ds / dn as f64 - os / on as f64) / mean.max(1e-9);
    if m == 4 {
        let (mut d, mut dn, mut a, mut an) = (0.0, 0usize, 0.0, 0usize);
        for (j, &v) in s.iter().enumerate() {
            match (j + 4 - phase) % 4 {
                0 => {
                    d += v;
                    dn += 1;
                }
                2 => {
                    a += v;
                    an += 1;
                }
                _ => {}
            }
        }
        if dn > 0 && an > 0 {
            sc += 0.5 * ((d / dn as f64 - a / an as f64) / mean.max(1e-9)).clamp(0.0, 1.0);
        }
    }
    sc
}

/// Estima compás y anacrusa. `fixed` = negras por compás ya decididas por el usuario (solo se
/// busca la anacrusa). Con poca evidencia devuelve 4 tiempos, sin anacrusa y confianza baja.
pub fn detect_meter(notes: &[RawNote], map: &BeatMap, fixed: Option<u32>, detect_pickup: bool, compound: bool) -> MeterGuess {
    let first = notes.iter().map(|n| n.start).fold(f64::MAX, f64::min);
    let k0 = (map.pos(first) + 0.25).floor() as i64;
    let s = beat_strengths(notes, map, k0);
    let default = MeterGuess { beats: fixed.unwrap_or(4), pickup: 0, confidence: 0.0 };
    if s.len() < 12 {
        return default;
    }
    let meters: Vec<u32> = match fixed {
        Some(m) => vec![m],
        None => vec![2, 3, 4],
    };
    // (puntuación, compás, fase)
    let mut hyp: Vec<(f64, u32, usize)> = Vec::new();
    for &m in &meters {
        if !(2..=4).contains(&m) {
            // compases largos: solo se busca la fase con el criterio de «un tiempo fuerte por compás»
            for ph in 0..m as usize {
                if !detect_pickup && ph != 0 {
                    continue;
                }
                hyp.push((downbeat_score_n(&s, m as usize, ph), m, ph));
            }
            continue;
        }
        for ph in 0..m as usize {
            if !detect_pickup && ph != 0 {
                continue;
            }
            let mut sc = downbeat_score(&s, m as usize, ph);
            // sesgos: en compás simple se prefiere 4/4; en compuesto, 6/8 (el 12/8 necesita evidencia)
            if m == 3 {
                sc -= METER_PRIOR_3;
            }
            if m == 2 && !compound {
                sc -= METER_PRIOR_2;
            }
            if m == 4 && compound {
                sc -= METER_PRIOR_2;
            }
            if ph == 0 {
                sc += PICKUP_PRIOR;
            }
            hyp.push((sc, m, ph));
        }
    }
    hyp.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    if std::env::var("NOVA_DEBUG_METER").is_ok() {
        eprintln!("strengths {:?}", &s[..s.len().min(14)]);
        for h in hyp.iter().take(6) {
            eprintln!("  hyp {:.3} m {} ph {}", h.0, h.1, h.2);
        }
    }
    let (best, m, ph) = hyp[0];
    // 2/4 y 4/4 con tiempos fuertes anidados (misma paridad de fase) no son rivales a efectos de la anacrusa
    let second = hyp.iter().skip(1).find(|h| !(h.1 != m && (h.1 == 2 || h.1 == 4) && (m == 2 || m == 4) && h.2 % 2 == ph % 2)).map_or(best - 1.0, |h| h.0);
    // en la fase, j ≡ ph (mod m) son los tiempos fuertes: la anacrusa son los ph tiempos previos
    MeterGuess { beats: m, pickup: ph as u32, confidence: ((best - second) / best.abs().max(0.2)).clamp(0.0, 1.0) }
}

fn downbeat_score_n(s: &[f64], m: usize, phase: usize) -> f64 {
    downbeat_score(s, m, phase)
}

/// Sesgo a favor del 4/4 frente al 3/4 y 2/4, y de «sin anacrusa» (calibrados en `docs/CALIBRACION.md`).
/// Confianza mínima (0..1) para fiarse de la detección; por debajo se usa 4/4 sin anacrusa.
const METER_MIN_CONFIDENCE: f64 = 0.15;
/// Peso extra por cada nota simultánea (acordes) al medir la fuerza de un pulso.
const METER_CHORD_GAIN: f64 = 0.15;
/// Parte mínima de los pulsos con notas que deben dividirse en tres para suponer compás compuesto.
const COMPOUND_MIN_SHARE: f64 = 0.5;
/// Umbral relativo (a P90 de la amplitud) del conjunto fijo de notas con el que se sigue el pulso.
const TRACK_REL: f32 = 0.5;
const METER_PRIOR_3: f64 = 0.10;
const METER_PRIOR_2: f64 = 0.15;
const PICKUP_PRIOR: f64 = 0.15;

/// Peso de cada inicio para el seguimiento de pulso: los acordes, los graves y las notas largas
/// suelen marcar los tiempos fuertes, así que cuentan más que una nota suelta y breve.
fn accent_weights(notes: &[RawNote]) -> Vec<(f64, f32)> {
    notes
        .iter()
        .map(|n| {
            let simultaneous = notes.iter().filter(|m| (m.start - n.start).abs() < 0.04).count() as f32;
            let mut w = n.amplitude * (1.0 + 0.5 * (simultaneous - 1.0).min(3.0));
            if n.pitch < 55 {
                w *= 1.3;
            }
            if n.end - n.start > 0.7 {
                w *= 1.3;
            }
            (n.start, w)
        })
        .collect()
}

/// El seguimiento flexible se usa solo si reduce el desajuste de los ataques con la rejilla por debajo de esta fracción.
const LAX_GAIN: f64 = 0.7;

/// El seguimiento flexible se descarta si más de esta fracción de sus intervalos salta más de un 12 % respecto al
/// anterior: un rubato real cambia el tempo de forma continua; unos pulsos que saltan (4 y 5 semicorcheas
/// alternas en BWV 847) solo se ajustan mejor porque siguen los ataques y producen quintillos falsos.
const LAX_MAX_JITTER: f64 = 0.2;

/// Fracción de intervalos entre pulsos consecutivos que cambian más de un 12 % respecto al anterior.
fn beat_jitter(beats: &[f64]) -> f64 {
    let d: Vec<f64> = beats.windows(2).map(|x| x[1] - x[0]).collect();
    if d.len() < 3 {
        return 0.0;
    }
    d.windows(2).filter(|x| (x[1] / x[0] - 1.0).abs() > 0.12).count() as f64 / (d.len() - 1) as f64
}

/// Desajuste mediano (en pulsos) de los ataques respecto a la rejilla de semicorcheas y tresillos de corchea
/// según un mapa de pulsos: cuanto menor, mejor explica el mapa el ritmo.
fn grid_residual(map: &BeatMap, notes: &[RawNote]) -> f64 {
    const GRID: [f64; 7] = [0.0, 0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 1.0];
    let mut d: Vec<f64> = notes
        .iter()
        .map(|n| {
            let p = map.pos(n.start);
            let f = p - p.floor();
            GRID.iter().map(|g| (f - g).abs()).fold(f64::MAX, f64::min)
        })
        .collect();
    if d.is_empty() {
        return 0.0;
    }
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    d[d.len() / 2]
}

pub fn notes_to_score(raw: &[RawNote], o: &QuantizeOptions) -> Result<ImportReport, String> {
    let mut warnings = Vec::new();
    let all = raw.len();
    let threshold = match o.min_relative {
        Some(r) => {
            let mut amps: Vec<f32> = raw.iter().map(|n| n.amplitude).collect();
            amps.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p90 = amps.get(amps.len() * 9 / 10).copied().unwrap_or(0.0);
            (r * p90).max(0.2)
        }
        None => o.min_amplitude,
    };
    let notes: Vec<RawNote> = raw
        .iter()
        .copied()
        .filter(|n| n.amplitude >= threshold && n.end > n.start && (21..=108).contains(&n.pitch))
        .collect();
    if notes.is_empty() {
        return Err("No se detectaron notas (prueba a bajar el umbral de amplitud).".into());
    }
    if all > notes.len() {
        warnings.push(format!("Se descartaron {} de {all} notas por amplitud baja (< {:.2}).", all - notes.len(), threshold));
    }

    let notes = if o.clean {
        let (clean, st) = clean_notes_with(&notes, &o.clean_params);
        if st.key_doubtful {
            warnings.push("La tonalidad estimada es dudosa: no se eliminaron notas cromáticas (revisa las alteraciones).".into());
        }
        if st.merged + st.harmonics + st.chromatic > 0 {
            warnings.push(format!(
                "Limpieza: {} fragmentos unidos, {} armónicos y {} notas cromáticas débiles eliminados.",
                st.merged, st.harmonics, st.chromatic
            ));
        }
        if clean.is_empty() {
            return Err("No quedaron notas tras la limpieza.".into());
        }
        clean
    } else {
        notes
    };

    let forced_compound = !o.auto_meter && o.time.den == 8 && o.time.num % 3 == 0 && o.time.num >= 6;
    // Pulso
    let onsets: Vec<(f64, f32)> = notes.iter().map(|n| (n.start, n.amplitude)).collect();
    let first = notes.iter().map(|n| n.start).fold(f64::MAX, f64::min);
    let last = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    // con `robust_tempo` el pulso se busca en un conjunto fijo de notas (las más fuertes de todas
    // las detectadas), de modo que el tempo no cambia al mover el umbral de amplitud
    let track_notes: Vec<RawNote> = if o.robust_tempo {
        let mut amps: Vec<f32> = raw.iter().map(|n| n.amplitude).collect();
        amps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p90 = amps.get(amps.len() * 9 / 10).copied().unwrap_or(0.0);
        let c: Vec<RawNote> = raw.iter().copied().filter(|n| n.amplitude >= TRACK_REL * p90 && n.end > n.start && (21..=108).contains(&n.pitch)).collect();
        if o.clean && !c.is_empty() { clean_notes_with(&c, &o.clean_params).0 } else { c }
    } else {
        notes.clone()
    };
    let track_notes = if track_notes.len() >= 24 { track_notes } else { notes.clone() };
    // Con muy pocas notas el seguimiento no es fiable: se usa tempo constante.
    let map = if o.follow_tempo && track_notes.len() >= 24 {
        let seed = o.tempo.map(|t| 60.0 / t.clamp(20.0, 300.0));
        let w = accent_weights(&track_notes);
        let rep = crate::beats::repetition(&track_notes.iter().map(|n| (n.start, n.pitch)).collect::<Vec<_>>());
        let track = |tight: f64| -> Vec<f64> {
            let mut beats = crate::beats::find_beats_hint(&w, seed, tight, rep);
            if forced_compound && seed.is_none() && beats.len() > 4 {
                // compás compuesto indicado sin tempo: el pulso es la negra con puntillo, ~1,5 veces el de la negra
                let mut d: Vec<f64> = beats.windows(2).map(|x| x[1] - x[0]).collect();
                d.sort_by(|a, b| a.partial_cmp(b).unwrap());
                beats = find_beats(&w, Some(1.5 * d[d.len() / 2]), tight);
            }
            beats
        };
        let mut beats = track(o.tightness);
        if let Some(lax) = o.lax_tightness {
            let lb = track(lax);
            if beats.len() > 4 && lb.len() > 4 {
                let rigid = grid_residual(&BeatMap::from_beats(beats.clone(), seed.unwrap_or(0.6)), &track_notes);
                let loose = grid_residual(&BeatMap::from_beats(lb.clone(), seed.unwrap_or(0.6)), &track_notes);
                if loose < LAX_GAIN * rigid && beat_jitter(&lb) <= LAX_MAX_JITTER {
                    warnings.push(format!("Tempo con variaciones marcadas (rubato): seguimiento flexible (desajuste con la rejilla {:.3} → {:.3} pulsos).", rigid, loose));
                    beats = lb;
                }
            }
        }
        BeatMap::from_beats(beats, seed.unwrap_or(0.6))
    } else {
        let est = estimate_beat(&onsets);
        let (bpm, phase) = match o.tempo {
            Some(t) => {
                let t = t.clamp(20.0, 300.0);
                (t, concentration(&onsets, t).1.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU * 60.0 / t)
            }
            None => (est.bpm, est.phase),
        };
        BeatMap::regular(phase, 60.0 / bpm, last)
    };
    let bpm = map.median_bpm();
    // ¿Compás compuesto? El pulso seguido es la negra con puntillo si casi todos los pulsos con
    // notas se dividen en tres (los tresillos de un compás simple son una minoría).
    let origin0 = (map.pos(first) + 0.25).floor();
    let prov: Vec<f64> = notes.iter().map(|n| ((map.pos(n.start) - origin0) * PPQ as f64).max(0.0)).collect();
    let compound = if forced_compound {
        true
    } else if o.compound && o.auto_meter && o.triplets {
        let tb = detect_tuplet_beats(&prov);
        let busy: std::collections::BTreeSet<u32> = prov.iter().map(|&t| (t / PPQ as f64) as u32).collect();
        let triple = tb.iter().filter(|(b, &k)| k == 3 && busy.contains(b)).count();
        busy.len() >= 8 && triple as f64 >= COMPOUND_MIN_SHARE * busy.len() as f64
    } else {
        false
    };

    // Compás y anacrusa (en compás compuesto el pulso es la negra con puntillo)
    let mut time = o.time;
    let mut pickup = 0u32;
    if o.auto_meter || o.auto_pickup {
        let fixed = if o.auto_meter {
            None
        } else if compound {
            Some(o.time.num / 3)
        } else if o.time.measure_ticks() % PPQ == 0 {
            Some(o.time.measure_ticks() / PPQ)
        } else {
            None
        };
        if o.auto_meter || fixed.is_some() {
            let g = detect_meter(&notes, &map, fixed, o.auto_pickup, compound);
            if g.confidence >= METER_MIN_CONFIDENCE {
                if o.auto_meter {
                    time = if compound { TimeSig { num: 3 * g.beats, den: 8 } } else { TimeSig { num: g.beats, den: 4 } };
                }
                pickup = g.pickup;
                warnings.push(format!(
                    "Compás {}/{} {} (confianza {:.0} %){}.",
                    time.num,
                    time.den,
                    if o.auto_meter { "detectado" } else { "indicado" },
                    g.confidence * 100.0,
                    if pickup > 0 { format!("; anacrusa de {pickup} tiempo(s): el primer compás empieza con silencios") } else { String::new() }
                ));
            } else if o.auto_meter {
                if compound {
                    time = TimeSig { num: 6, den: 8 };
                }
                warnings.push(format!("No se pudo detectar el compás con fiabilidad (confianza {:.0} %): se supone {}/{}. Cámbialo si no es el correcto.", g.confidence * 100.0, time.num, time.den));
            }
        } else if compound {
            time = o.time;
        }
    }
    if compound && !(time.den == 8 && time.num % 3 == 0) {
        time = TimeSig { num: 6, den: 8 };
    }
    if compound {
        warnings.push("Compás compuesto: el pulso es la negra con puntillo dividida en corcheas (sin grupos irregulares).".into());
    }
    let beats_per_bar = if compound { (time.num / 3).max(1) } else { (time.measure_ticks() / PPQ).max(1) };
    // origen de la rejilla: el primer pulso de compás anterior o igual a la primera nota
    let origin = origin0 - if pickup > 0 { (beats_per_bar - pickup % beats_per_bar) as f64 } else { 0.0 };
    warnings.push(format!(
        "Tempo {}: {:.1} {}/min.{}",
        if o.tempo.is_some() { "indicado" } else { "estimado" },
        bpm,
        if compound { "negras con puntillo" } else { "negras" },
        if o.follow_tempo { " Se siguen las variaciones de tempo." } else { " Tempo constante: no se sigue el rubato." }
    ));

    // Tonalidad
    let fifths = estimate_fifths(&notes);
    let sharps = fifths >= 0;

    // Cuantización
    let to_ticks = |t: f64| -> f64 { (map.pos(t) - origin) * PPQ as f64 };
    let raw_ticks: Vec<(f64, f64)> = notes.iter().map(|n| (to_ticks(n.start).max(0.0), to_ticks(n.end).max(0.0))).collect();
    let tuplet_beats: HashMap<u32, u8> = if o.triplets && !compound {
        detect_tuplet_beats(&raw_ticks.iter().map(|r| r.0).collect::<Vec<_>>())
    } else {
        HashMap::new()
    };
    let mut on_grid = 0usize;
    let mut q: Vec<Q> = Vec::new();
    for (n, &(ts, te)) in notes.iter().zip(&raw_ticks) {
        if compound {
            // rejilla de 1/6 de pulso (semicorchea); los ticks internos (480 por pulso) pasan a reales ×3/2
            let g = (PPQ / 6) as f64;
            let s = (ts / g).round() as u32 * (PPQ / 6);
            if (ts - s as f64).abs() < 0.15 * (PPQ / 3) as f64 {
                on_grid += 1;
            }
            let e = ((te / g).round() as u32 * (PPQ / 6)).max(s + PPQ / 6);
            q.push(Q { s: s * 3 / 2, e: e * 3 / 2, pitch: n.pitch });
            continue;
        }
        let s = snap(ts, &tuplet_beats);
        let unit = slot_ticks(s / PPQ, &tuplet_beats);
        if (ts - s as f64).abs() < 0.15 * unit as f64 {
            on_grid += 1;
        }
        let e = snap(te, &tuplet_beats).max(s + unit);
        q.push(Q { s, e, pitch: n.pitch });
    }
    let end_tick = q.iter().map(|x| x.e).max().unwrap();
    let mlen = time.measure_ticks();
    if end_tick / mlen >= MAX_MEASURES {
        return Err(format!("La pieza supera {MAX_MEASURES} compases con el tempo estimado."));
    }
    let on_grid_pct = on_grid as f64 * 100.0 / q.len() as f64;
    if on_grid_pct < 60.0 {
        warnings.push(format!("Solo el {on_grid_pct:.0} % de las notas cae en la rejilla de semicorcheas: el ritmo es irregular o el pulso no es el adecuado. Prueba a indicar el tempo (o la mitad / el doble)."));
    }
    if !tuplet_beats.is_empty() {
        let count = |n: u8| tuplet_beats.values().filter(|&&v| v == n).count();
        let mut parts = Vec::new();
        for (n, name) in [(3u8, "tresillos"), (5, "quintillos"), (6, "seisillos")] {
            if count(n) > 0 {
                parts.push(format!("{} de {name}", count(n)));
            }
        }
        warnings.push(format!("Se detectaron pulsos con grupos irregulares: {}.", parts.join(", ")));
    }
    if !(o.auto_meter || o.auto_pickup) {
        warnings.push(format!("Compás {}/{} supuesto; la primera nota se coloca en el primer tiempo (sin anacrusa).", time.num, time.den));
    }
    warnings.push("Cuantizado a semicorcheas.".into());

    // Pentagramas
    let low: Vec<Q> = q.iter().copied().filter(|x| x.pitch < SPLIT_PITCH).collect();
    let high: Vec<Q> = q.iter().copied().filter(|x| x.pitch >= SPLIT_PITCH).collect();
    let minority = low.len().min(high.len());
    let mut staves: Vec<(String, Clef, Vec<Q>)> = Vec::new();
    if minority * 12 < q.len() {
        let mean = q.iter().map(|x| x.pitch as f64).sum::<f64>() / q.len() as f64;
        let clef = if mean < 55.0 { Clef::Bass } else { Clef::Treble };
        staves.push(("Instrumento".into(), clef, q.clone()));
    } else {
        staves.push(("Mano derecha".into(), Clef::Treble, high));
        staves.push(("Mano izquierda".into(), Clef::Bass, low));
    }

    let mut score = Score::new(&o.title, time, fifths, 1);
    score.tempo = (if compound { bpm * 1.5 } else { bpm }).round().clamp(20.0, 300.0) as u32;
    score.parts.clear();
    let mut dropped_voices = false;
    for (name, clef, notes) in staves {
        let voices = allocate_voices(notes, o.max_voices.clamp(1, crate::MAX_VOICES), o.legato_ticks, &tuplet_beats);
        let mut part = Part::new(name, clef, o.program, vec![Measure::empty(mlen)]);
        part.percussion = false;
        score.parts.push(part);
        let pi = score.parts.len() - 1;
        for (vi, v) in voices.iter().enumerate() {
            // pulsos con grupo irregular de esta voz: pulso → (ranura, nº de ranuras, alturas, ligada)
            let mut beats: BTreeMap<u32, Vec<(u32, u32, Vec<Pitch>, bool)>> = BTreeMap::new();
            for g in v {
                let mut ps: Vec<Pitch> = g.pitches.iter().map(|&m| Pitch::from_midi(m as i32, sharps)).collect();
                ps.sort_by_key(|p| p.midi());
                let mut pos = g.s;
                while pos < g.e {
                    let beat = pos / PPQ;
                    if let Some(&slots) = tuplet_beats.get(&beat) {
                        let unit = PPQ / slots as u32;
                        let seg_end = g.e.min((beat + 1) * PPQ);
                        let slot = ((pos - beat * PPQ) as f64 / unit as f64).round() as u32;
                        let n = (((seg_end - pos) as f64 / unit as f64).round() as u32).max(1);
                        let tie = seg_end < g.e;
                        if n >= slots as u32 || slot + n > slots as u32 {
                            // llena el pulso: una negra normal, sin grupo irregular
                            if score.put_abs_voice_tied(pi, vi, pos, seg_end - pos, &ps, tie).is_err() {
                                dropped_voices = true;
                            }
                        } else {
                            beats.entry(beat).or_default().push((slot, n, ps.clone(), tie));
                        }
                        pos = seg_end;
                    } else {
                        // tramo recto hasta el siguiente pulso con grupo irregular
                        let mut seg_end = g.e;
                        let mut b = beat + 1;
                        while b * PPQ < g.e {
                            if tuplet_beats.contains_key(&b) {
                                seg_end = b * PPQ;
                                break;
                            }
                            b += 1;
                        }
                        if score.put_abs_voice_tied(pi, vi, pos, seg_end - pos, &ps, seg_end < g.e).is_err() {
                            dropped_voices = true;
                        }
                        pos = seg_end;
                    }
                }
            }
            for (beat, items) in beats {
                if score.put_tuplet_beat(pi, vi, beat * PPQ, tuplet_beats[&beat] as u32, &items).is_err() {
                    dropped_voices = true;
                }
            }
        }
    }
    if dropped_voices {
        warnings.push("Algunas notas no pudieron colocarse y se omitieron.".into());
    }
    let max_len = score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(1);
    let lens: Vec<u32> = (0..max_len).map(|i| score.measure_len(i)).collect();
    for p in &mut score.parts {
        while p.measures.len() < max_len {
            p.measures.push(Measure::empty(lens[p.measures.len()]));
        }
    }
    Ok(ImportReport { score, warnings })
}

#[cfg(test)]
mod tests {
    #[test]
    fn beat_jitter_separates_rubato_from_jumping_beats() {
        // rubato suave: intervalos que cambian poco
        let mut t = 0.0;
        let smooth: Vec<f64> = (0..40).map(|i| { t += 0.5 + 0.04 * (i as f64 * 0.3).sin(); t }).collect();
        assert!(beat_jitter(&smooth) < 0.05);
        // pulsos que alternan 4 y 5 semicorcheas (0,42 / 0,52 s)
        let mut t = 0.0;
        let jumpy: Vec<f64> = (0..40).map(|i| { t += if i % 2 == 0 { 0.42 } else { 0.52 }; t }).collect();
        assert!(beat_jitter(&jumpy) > LAX_MAX_JITTER);
    }

    use super::*;
    use crate::io::sounds;

    fn n_note(start_beats: f64, dur_beats: f64, pitch: u8, bpm: f64) -> RawNote {
        n(start_beats, dur_beats, pitch, bpm)
    }

    fn n(start_beats: f64, dur_beats: f64, pitch: u8, bpm: f64) -> RawNote {
        let b = 60.0 / bpm;
        RawNote { start: 1.3 + start_beats * b, end: 1.3 + (start_beats + dur_beats) * b - 0.02, pitch, amplitude: 0.8, onset: true }
    }

    #[test]
    fn tempo_is_recovered() {
        for bpm in [72.0, 100.0, 120.0, 138.5] {
            let notes: Vec<(f64, f32)> = (0..64).map(|i| (1.3 + i as f64 * 60.0 / bpm * if i % 3 == 0 { 1.0 } else { 1.0 }, 0.8)).collect();
            let est = estimate_beat(&notes);
            assert!((est.bpm - bpm).abs() < 0.3, "{bpm} -> {}", est.bpm);
            assert!(est.confidence > 0.9);
        }
    }

    #[test]
    fn tempo_with_eighths_and_swing_free_rhythm() {
        let bpm = 96.0;
        let mut v = Vec::new();
        for i in 0..40 {
            let b = i as f64 * 0.5 + if i % 5 == 4 { 0.5 } else { 0.0 };
            v.push((2.0 + b * 60.0 / bpm, 0.8f32));
        }
        let est = estimate_beat(&v);
        assert!((est.bpm - bpm).abs() < 0.5, "{}", est.bpm);
    }

    #[test]
    fn key_of_c_major_and_a_minor_and_d_major() {
        let scale = |tonic: u8, steps: &[u8]| -> Vec<RawNote> {
            steps.iter().enumerate().map(|(i, s)| RawNote { start: i as f64, end: i as f64 + 0.9, pitch: tonic + s, amplitude: 0.8, onset: true }).collect()
        };
        let maj = [0, 2, 4, 5, 7, 9, 11, 12, 0, 4, 7, 12, 7, 4, 0];
        assert_eq!(estimate_fifths(&scale(60, &maj)), 0);
        assert_eq!(estimate_fifths(&scale(62, &maj)), 2);
        let min = [0, 2, 3, 5, 7, 8, 11, 12, 0, 3, 7, 12, 7, 3, 0, 0];
        assert_eq!(estimate_fifths(&scale(57, &min)), 0);
    }

    #[test]
    fn melody_becomes_measures_with_right_durations() {
        let bpm = 100.0;
        // 4/4: negra, negra, blanca | corchea x4, negra, negra
        let raw = vec![
            n(0.0, 1.0, 72, bpm),
            n(1.0, 1.0, 74, bpm),
            n(2.0, 2.0, 76, bpm),
            n(4.0, 0.5, 77, bpm),
            n(4.5, 0.5, 76, bpm),
            n(5.0, 0.5, 74, bpm),
            n(5.5, 0.5, 72, bpm),
            n(6.0, 1.0, 71, bpm),
            n(7.0, 1.0, 72, bpm),
        ];
        let r = notes_to_score(&raw, &QuantizeOptions::default()).unwrap();
        assert_eq!(r.score.parts.len(), 1);
        assert!((r.score.tempo as i32 - 100).abs() <= 1);
        let m0 = &r.score.parts[0].measures[0];
        let lens: Vec<u32> = m0.events.iter().map(|e| e.ticks).collect();
        assert_eq!(lens, vec![480, 480, 960]);
        let m1 = &r.score.parts[0].measures[1];
        let lens: Vec<u32> = m1.events.iter().map(|e| e.ticks).collect();
        assert_eq!(lens, vec![240, 240, 240, 240, 480, 480]);
    }

    #[test]
    fn two_hands_and_voices() {
        let bpm = 90.0;
        let mut raw = vec![n(0.0, 4.0, 48, bpm), n(4.0, 4.0, 43, bpm)]; // bajo largo
        for i in 0..8 {
            raw.push(n(i as f64, 1.0, [64, 66, 67, 69, 71, 69, 67, 66][i], bpm));
        }
        let r = notes_to_score(&raw, &QuantizeOptions::default()).unwrap();
        assert_eq!(r.score.parts.len(), 2);
        assert_eq!(r.score.parts[1].clef, Clef::Bass);
        // la mano izquierda conserva las redondas
        assert_eq!(r.score.parts[1].measures[0].events[0].ticks, 1920);
        // el sonido total conserva todas las notas
        assert_eq!(sounds(&r.score).len(), 10);
    }

    #[test]
    fn overlapping_melody_over_held_note_uses_second_voice() {
        let bpm = 100.0;
        let mut raw = vec![n(0.0, 4.0, 67, bpm)]; // nota larga aguda
        for i in 0..4 {
            raw.push(n(i as f64, 1.0, [60, 62, 64, 62][i], bpm)); // melodía grave, misma mano
        }
        let q = QuantizeOptions { min_amplitude: 0.0, min_relative: None, max_voices: 2, ..Default::default() };
        let r = notes_to_score(&raw, &q).unwrap();
        let sp = &r.score.parts;
        // una mano derecha con 2 voces (el 67 sostenido arriba, la melodía abajo)
        let right = &sp[0];
        assert_eq!(right.measures[0].voices.len(), 1);
        assert_eq!(right.measures[0].events.len(), 1);
        assert_eq!(right.measures[0].events[0].ticks, 1920);
    }

    #[test]
    fn detects_triplet_beats_from_onset_phases() {
        // pulso 0: tres corcheas de tresillo; pulso 1: negra; pulso 2: dos corcheas rectas
        let ticks = [0.0, 160.0, 320.0, 480.0, 960.0, 1200.0];
        let set = detect_tuplet_beats(&ticks);
        assert_eq!(set.get(&0), Some(&3));
        assert!(!set.contains_key(&1) && !set.contains_key(&2));
        // una racha de cuatro pulsos de tresillos con ±10 ticks de ruido se detecta entera
        let jitter = [6.0, -10.0, 10.0, -8.0, 9.0, -10.0, 7.0, 10.0, -6.0, 9.0, -9.0, 8.0];
        let noisy: Vec<f64> = (0..12).map(|i| (i / 3) as f64 * 480.0 + (i % 3) as f64 * 160.0 + jitter[i]).collect();
        let found = detect_tuplet_beats(&noisy);
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(found.values().all(|&v| v == 3));
        // negra + corchea de tresillo en tres pulsos seguidos
        let swing = [0.0, 320.0, 480.0, 800.0, 960.0, 1280.0];
        assert_eq!(detect_tuplet_beats(&swing), [(0u32, 3u8), (1, 3), (2, 3)].into_iter().collect());
        // semicorcheas rectas: ninguno
        let straight = [0.0, 120.0, 240.0, 360.0, 480.0, 600.0, 720.0, 840.0];
        assert!(detect_tuplet_beats(&straight).is_empty());
    }

    #[test]
    fn triplets_need_more_evidence_when_timing_is_unstable() {
        // tresillos limpios repetidos durante 40 pulsos: se detectan todos
        let clean: Vec<f64> = (0..40).flat_map(|b| (0..3).map(move |k| b as f64 * 480.0 + k as f64 * 160.0)).collect();
        assert_eq!(detect_tuplet_beats(&clean).len(), 40);
        // inicios con fase aleatoria (tempo libre): casi ningún pulso debe salir como tresillo
        let mut seed = 99u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f64 / (1u32 << 24) as f64
        };
        let random: Vec<f64> = (0..40).flat_map(|b| (0..3).map(move |k| (b, k))).map(|(b, _)| b as f64 * 480.0 + rnd() * 480.0).collect();
        let found = detect_tuplet_beats(&random).len();
        assert!(found <= 8, "{found} pulsos de tresillos en inicios aleatorios");
        // tresillos con mucho ruido (±45 ticks ≈ ±0,09 pulso): menos que con ruido pequeño
        let noisy: Vec<f64> = (0..40).flat_map(|b| (0..3).map(move |k| (b, k))).map(|(b, k)| b as f64 * 480.0 + k as f64 * 160.0 + (rnd() - 0.5) * 90.0).collect();
        assert!(detect_tuplet_beats(&noisy).len() < 40);
    }

    #[test]
    fn doubtful_key_keeps_weak_chromatic_notes() {
        // escala de do mayor muy clara + una nota débil y corta de do♯: se quita
        let mut notes: Vec<RawNote> = [60, 62, 64, 65, 67, 69, 71, 72, 71, 69, 67, 65, 64, 62, 60, 67, 64, 60, 62, 64]
            .iter()
            .enumerate()
            .map(|(i, &p)| RawNote { start: i as f64 * 0.5, end: i as f64 * 0.5 + 0.45, pitch: p, amplitude: 0.8, onset: true })
            .collect();
        notes.push(RawNote { start: 4.0, end: 4.2, pitch: 61, amplitude: 0.3, onset: true });
        let (clean, st) = clean_notes(&notes);
        assert!(!st.key_doubtful, "{st:?}");
        assert_eq!(st.chromatic, 1);
        assert!(clean.iter().all(|n| n.pitch != 61));
        // histograma casi plano (todas las alturas parecidas): tonalidad dudosa, no se quita nada
        let flat: Vec<RawNote> = (0..48)
            .map(|i| RawNote { start: i as f64 * 0.3, end: i as f64 * 0.3 + 0.25, pitch: 60 + (i % 12) as u8, amplitude: if i == 5 { 0.2 } else { 0.8 }, onset: true })
            .collect();
        let (clean, st) = clean_notes(&flat);
        assert!(st.key_doubtful);
        assert_eq!(st.chromatic, 0);
        assert_eq!(clean.len(), flat.len());
    }

    #[test]
    fn triplet_eighths_are_written_as_tuplets() {
        let bpm = 100.0;
        let b = 60.0 / bpm;
        let mut raw = Vec::new();
        // dos compases de tresillos de corchea (24 notas) y dos de negras
        for i in 0..24 {
            let beat = i as f64 / 3.0;
            raw.push(RawNote { start: 1.3 + beat * b, end: 1.3 + (beat + 1.0 / 3.0) * b - 0.02, pitch: [60, 62, 64][i % 3] + 5 * ((i / 12) as u8), amplitude: 0.8, onset: true });
        }
        for k in 0..8 {
            raw.push(n(8.0 + k as f64, 1.0, 67 + (k % 3) as u8, bpm));
        }
        let q = QuantizeOptions { tempo: Some(bpm), min_relative: None, min_amplitude: 0.0, clean: false, ..Default::default() };
        let r = notes_to_score(&raw, &q).unwrap();
        let part = &r.score.parts[0];
        let m0: Vec<&crate::Event> = part.measures[0].events.iter().collect();
        assert_eq!(m0.len(), 12, "{:?}", m0.iter().map(|e| e.ticks).collect::<Vec<_>>());
        assert!(m0.iter().all(|e| e.ticks == 160 && e.tuplet.is_some()));
        let m2: Vec<u32> = part.measures[2].events.iter().map(|e| e.ticks).collect();
        assert_eq!(m2, vec![480, 480, 480, 480]);
        assert!(part.measures[2].events.iter().all(|e| e.tuplet.is_none()));
        assert_eq!(sounds(&r.score).len(), 32);
        assert!(r.warnings.iter().any(|w| w.contains("8 de tresillos")), "{:?}", r.warnings);
        // sin detección de tresillos se cuantiza a semicorcheas (no hay grupos)
        let q2 = QuantizeOptions { triplets: false, ..q.clone() };
        let r2 = notes_to_score(&raw, &q2).unwrap();
        assert!(r2.score.parts[0].measures[0].events.iter().all(|e| e.tuplet.is_none()));
    }

    #[test]
    fn detects_quintuplets_and_sextuplets_but_prefers_triplets() {
        let group = |n: u32, beats: u32| -> Vec<f64> {
            (0..beats).flat_map(|b| (0..n).map(move |k| b as f64 * 480.0 + k as f64 * 480.0 / n as f64)).collect()
        };
        let five = detect_tuplet_beats(&group(5, 12));
        assert_eq!(five.len(), 12);
        assert!(five.values().all(|&v| v == 5), "{five:?}");
        let six = detect_tuplet_beats(&group(6, 12));
        assert_eq!(six.len(), 12);
        assert!(six.values().all(|&v| v == 6), "{six:?}");
        // tres por pulso siguen siendo tresillos, no seisillos (la rejilla de 6 incluye la de 3)
        assert!(detect_tuplet_beats(&group(3, 12)).values().all(|&v| v == 3));
        // cuatro por pulso (semicorcheas): ninguno
        assert!(detect_tuplet_beats(&group(4, 12)).is_empty());
    }

    #[test]
    fn quintuplets_and_sextuplets_are_written_as_tuplets() {
        let bpm = 100.0;
        let b = 60.0 / bpm;
        for (n, normal) in [(5u32, 4u8), (6, 4)] {
            let mut raw = Vec::new();
            // dos compases de grupos de n notas por pulso y dos de negras
            for beat in 0..8u32 {
                for k in 0..n {
                    let at = beat as f64 + k as f64 / n as f64;
                    raw.push(RawNote { start: 1.3 + at * b, end: 1.3 + (at + 1.0 / n as f64) * b - 0.02, pitch: 60 + ((beat * n + k) % 7) as u8, amplitude: 0.8, onset: true });
                }
            }
            for k in 0..8 {
                raw.push(n_note(8.0 + k as f64, 1.0, 67 + (k % 3) as u8, bpm));
            }
            let q = QuantizeOptions { tempo: Some(bpm), min_relative: None, min_amplitude: 0.0, clean: false, ..Default::default() };
            let r = notes_to_score(&raw, &q).unwrap();
            let slot = 480 / n;
            let m0 = &r.score.parts[0].measures[0].events;
            assert_eq!(m0.len(), 4 * n as usize, "n={n}: {:?}", m0.iter().map(|e| e.ticks).collect::<Vec<_>>());
            assert!(m0.iter().all(|e| e.ticks == slot && e.tuplet.map_or(false, |t| t.actual as u32 == n && t.normal == normal)), "n={n}");
            assert!(r.score.parts[0].measures[2].events.iter().all(|e| e.tuplet.is_none() && e.ticks == 480));
            assert_eq!(sounds(&r.score).len(), 8 * n as usize + 8, "n={n}");
            let word = if n == 5 { "quintillos" } else { "seisillos" };
            assert!(r.warnings.iter().any(|w| w.contains(word)), "{:?}", r.warnings);
        }
    }

    #[test]
    fn long_note_into_a_triplet_beat_is_split_and_tied() {
        let bpm = 100.0;
        let b = 60.0 / bpm;
        let mut raw = Vec::new();
        // pulso 0: negra; pulso 1: tres corcheas de tresillo (la primera es una nota larga que
        // viene de la negra anterior con otra altura); se repite en pulso 2-3
        for rep in 0..8 {
            let base = rep as f64 * 2.0;
            raw.push(n(base, 1.0, 64, bpm));
            for k in 0..3 {
                raw.push(RawNote { start: 1.3 + (base + 1.0 + k as f64 / 3.0) * b, end: 1.3 + (base + 1.0 + (k as f64 + 1.0) / 3.0) * b - 0.02, pitch: 67 + k as u8, amplitude: 0.8, onset: true });
            }
        }
        let q = QuantizeOptions { tempo: Some(bpm), min_relative: None, min_amplitude: 0.0, clean: false, ..Default::default() };
        let r = notes_to_score(&raw, &q).unwrap();
        assert_eq!(sounds(&r.score).len(), 32);
        let m0 = &r.score.parts[0].measures[0].events;
        assert_eq!(m0.iter().map(|e| (e.start, e.ticks, e.tuplet.is_some())).collect::<Vec<_>>(), vec![(0, 480, false), (480, 160, true), (640, 160, true), (800, 160, true), (960, 480, false), (1440, 160, true), (1600, 160, true), (1760, 160, true)]);
    }

    #[test]
    fn overlong_final_note_is_snapped_to_a_simple_value() {
        let bpm = 100.0;
        let b = 60.0 / bpm;
        // negra que el modelo alarga a 1,25 negras y sigue silencio
        let raw = vec![n(0.0, 1.0, 60, bpm), n(1.0, 1.0, 62, bpm), RawNote { start: 1.3 + 2.0 * b, end: 1.3 + 3.25 * b, pitch: 64, amplitude: 0.8, onset: true }];
        let r = notes_to_score(&raw, &QuantizeOptions::default()).unwrap();
        let evs = &r.score.parts[0].measures[0].events;
        assert_eq!(evs[2].ticks, 480, "{:?}", evs.iter().map(|e| e.ticks).collect::<Vec<_>>());
    }

    #[test]
    fn low_amplitude_notes_are_dropped_and_empty_is_error() {
        let bpm = 100.0;
        let mut weak = n(0.0, 1.0, 60, bpm);
        weak.amplitude = 0.1;
        assert!(notes_to_score(&[weak], &QuantizeOptions::default()).is_err());
        let raw = vec![weak, n(1.0, 1.0, 62, bpm), n(2.0, 1.0, 64, bpm), n(3.0, 1.0, 65, bpm)];
        let r = notes_to_score(&raw, &QuantizeOptions::default()).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("descartaron 1 de 4")));
    }

    /// Pieza con bajo fuerte en el primer tiempo y acordes suaves en el resto, empezando `skip` tiempos tarde.
    fn metered(beats_per_bar: usize, bars: usize, skip: usize, bpm: f64) -> Vec<RawNote> {
        let b = 60.0 / bpm;
        let mut v = Vec::new();
        for bar in 0..bars {
            let bass = [40u8, 43, 45, 38][bar % 4];
            for k in 0..beats_per_bar {
                let beat = bar * beats_per_bar + k;
                if beat < skip {
                    continue;
                }
                let t0 = 1.3 + beat as f64 * b;
                if k == 0 {
                    v.push(RawNote { start: t0, end: t0 + b * 0.95, pitch: bass, amplitude: 0.95, onset: true });
                    v.push(RawNote { start: t0, end: t0 + b * 0.95, pitch: bass + 24, amplitude: 0.8, onset: true });
                } else {
                    for p in [60u8, 64, 67] {
                        v.push(RawNote { start: t0, end: t0 + b * 0.9, pitch: p + (bar % 3) as u8, amplitude: 0.5, onset: true });
                    }
                }
            }
        }
        v
    }

    fn meter_opts(bpm: f64) -> QuantizeOptions {
        QuantizeOptions { auto_meter: true, auto_pickup: true, tempo: Some(bpm), min_relative: None, min_amplitude: 0.0, clean: false, triplets: false, ..Default::default() }
    }

    #[test]
    fn detects_waltz_and_common_time_without_pickup() {
        let r = notes_to_score(&metered(3, 24, 0, 96.0), &meter_opts(96.0)).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (3, 4), "{:?}", r.warnings);
        assert_eq!(sounds(&r.score)[0].start, 0);
        let r = notes_to_score(&metered(4, 24, 0, 96.0), &meter_opts(96.0)).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (4, 4), "{:?}", r.warnings);
        assert_eq!(sounds(&r.score)[0].start, 0);
    }

    #[test]
    fn detects_pickup_beats() {
        // 4/4 que empieza 1 tiempo antes del compás (tiempo 4) y 3/4 que empieza 2 tiempos antes (tiempo 2)
        let r = notes_to_score(&metered(4, 24, 3, 96.0), &meter_opts(96.0)).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (4, 4), "{:?}", r.warnings);
        assert_eq!(sounds(&r.score)[0].start, 3 * 480, "{:?}", r.warnings);
        let r = notes_to_score(&metered(3, 24, 1, 96.0), &meter_opts(96.0)).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (3, 4), "{:?}", r.warnings);
        assert_eq!(sounds(&r.score)[0].start, 480, "{:?}", r.warnings);
    }

    #[test]
    fn fixed_meter_still_detects_pickup_and_flat_music_keeps_default() {
        let o = QuantizeOptions { auto_meter: false, time: TimeSig { num: 4, den: 4 }, ..meter_opts(96.0) };
        let r = notes_to_score(&metered(4, 24, 3, 96.0), &o).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (4, 4));
        assert_eq!(sounds(&r.score)[0].start, 3 * 480);
        // pulsos todos iguales: sin evidencia → 4/4 y sin anacrusa
        let flat: Vec<RawNote> = (0..60).map(|i| RawNote { start: 1.3 + i as f64 * 0.625, end: 1.3 + i as f64 * 0.625 + 0.5, pitch: 60, amplitude: 0.8, onset: true }).collect();
        let r = notes_to_score(&flat, &meter_opts(96.0)).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (4, 4));
        assert_eq!(sounds(&r.score)[0].start, 0);
    }

    /// Compás compuesto: `beats` pulsos de negra con puntillo por compás, tres corcheas por pulso;
    /// bajo fuerte en el primer pulso (y medio en el tercero de 12/8), empezando `skip` pulsos tarde.
    fn compound_piece(beats: usize, bars: usize, skip: usize, bpm: f64) -> Vec<RawNote> {
        let b = 60.0 / bpm; // un pulso = negra con puntillo
        let mut v = Vec::new();
        for bar in 0..bars {
            let bass = [40u8, 43, 45, 38][bar % 4];
            for k in 0..beats {
                let beat = bar * beats + k;
                if beat < skip {
                    continue;
                }
                for e in 0..3 {
                    let t0 = 1.3 + (beat as f64 + e as f64 / 3.0) * b;
                    let (pitch, amp) = if e == 0 && k == 0 {
                        (bass, 0.95)
                    } else if e == 0 && beats == 4 && k == 2 {
                        (bass, 0.6)
                    } else {
                        ([60u8, 64, 67, 64][(bar + k + e) % 4], 0.5)
                    };
                    v.push(RawNote { start: t0, end: t0 + b / 3.0 * 0.9, pitch, amplitude: amp, onset: true });
                }
            }
        }
        v
    }

    fn compound_opts(bpm: f64) -> QuantizeOptions {
        QuantizeOptions { compound: true, triplets: true, ..meter_opts(bpm) }
    }

    #[test]
    fn detects_six_eight_and_twelve_eight() {
        for (beats, num) in [(2usize, 6u32), (4, 12)] {
            let r = notes_to_score(&compound_piece(beats, 24, 0, 60.0), &compound_opts(60.0)).unwrap();
            assert_eq!((r.score.time.num, r.score.time.den), (num, 8), "{:?}", r.warnings);
            assert_eq!(r.score.tempo, 90); // negras por minuto: 60 pulsos de negra con puntillo × 1,5
            let s = sounds(&r.score);
            // las corcheas caen cada 240 ticks reales, sin grupos irregulares
            assert!(s.iter().all(|x| x.start % 240 == 0 && x.ticks % 240 == 0), "{:?}", s.iter().take(8).map(|x| (x.start, x.ticks)).collect::<Vec<_>>());
            assert_eq!(s.iter().map(|x| x.start).min(), Some(0));
            assert!(r.score.parts.iter().flat_map(|p| p.measures.iter()).all(|m| m.events.iter().all(|e| e.tuplet.is_none())));
        }
    }

    #[test]
    fn compound_pickup_and_fixed_meter() {
        // empieza un pulso antes del compás en 6/8: anacrusa de 1 pulso = 720 ticks de silencio inicial
        let r = notes_to_score(&compound_piece(2, 24, 1, 60.0), &compound_opts(60.0)).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (6, 8), "{:?}", r.warnings);
        assert_eq!(sounds(&r.score).iter().map(|x| x.start).min(), Some(720), "{:?}", r.warnings);
        // 12/8 indicado por el usuario, sin autodetección del compás
        let o = QuantizeOptions { auto_meter: false, time: TimeSig { num: 12, den: 8 }, ..compound_opts(60.0) };
        let r = notes_to_score(&compound_piece(4, 24, 0, 60.0), &o).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (12, 8));
        assert!(sounds(&r.score).iter().all(|x| x.start % 240 == 0));
    }

    #[test]
    fn simple_meter_with_eighths_is_not_compound() {
        let r = notes_to_score(&metered(4, 24, 0, 96.0), &compound_opts(96.0)).unwrap();
        assert_eq!((r.score.time.num, r.score.time.den), (4, 4), "{:?}", r.warnings);
    }

    #[test]
    fn robust_tempo_does_not_depend_on_the_amplitude_threshold() {
        // compases de 4 tiempos a 96 con negras fuertes y corcheas suaves (amplitudes 0,9 / 0,45 / 0,25)
        let b = 60.0 / 96.0;
        let mut v = Vec::new();
        for bar in 0..20 {
            for k in 0..4 {
                let t0 = 1.3 + (bar * 4 + k) as f64 * b;
                v.push(RawNote { start: t0, end: t0 + b * 0.9, pitch: if k == 0 { 40 } else { 64 }, amplitude: if k == 0 { 0.9 } else { 0.6 }, onset: true });
                v.push(RawNote { start: t0 + b / 2.0, end: t0 + b * 0.95, pitch: 67 + (k as u8 % 2), amplitude: if (bar + k) % 2 == 0 { 0.45 } else { 0.25 }, onset: true });
            }
        }
        let tempo_with = |rel: f32| {
            let o = QuantizeOptions { min_relative: Some(rel), robust_tempo: true, clean: false, triplets: false, ..Default::default() };
            notes_to_score(&v, &o).unwrap().score.tempo
        };
        let reference = tempo_with(0.3);
        for rel in [0.4, 0.5, 0.65, 0.8] {
            assert_eq!(tempo_with(rel), reference, "umbral {rel}");
        }
        assert!((reference as i32 - 96).abs() <= 2 || (reference as i32 - 192).abs() <= 4 || (reference as i32 - 48).abs() <= 2, "{reference}");
    }

    /// Negras fuertes + corcheas suaves a 100/min; con `rubato` > 0 el tempo oscila (periodo 16 pulsos).
    fn rubato_piece(rubato: f64) -> Vec<RawNote> {
        rubato_piece_p(rubato, 16.0)
    }

    fn rubato_piece_p(rubato: f64, period: f64) -> Vec<RawNote> {
        let (mut t, mut v, step) = (1.3f64, Vec::new(), 1.0 / 8.0);
        let mut beat = 0.0f64;
        while beat < 80.0 {
            let bpm = 100.0 * (1.0 + rubato * (std::f64::consts::TAU * beat / period).sin());
            let k = beat as u32;
            if (beat * 8.0).round() as u32 % 8 == 0 {
                v.push(RawNote { start: t, end: t + 0.4, pitch: if k % 4 == 0 { 40 } else { 64 }, amplitude: 0.9, onset: true });
            }
            if (beat * 8.0).round() as u32 % 8 == 4 {
                v.push(RawNote { start: t, end: t + 0.25, pitch: 67 + (k % 2) as u8, amplitude: 0.5, onset: true });
            }
            t += step * 60.0 / bpm;
            beat += step;
        }
        v
    }

    #[test]
    fn lax_tracking_follows_rubato_and_is_not_used_for_steady_tempo() {
        let on_grid = |r: &ImportReport| {
            let s = sounds(&r.score);
            let t0 = s.iter().map(|x| x.start).min().unwrap();
            s.iter().filter(|x| (x.start - t0) % 240 == 0).count() as f64 / s.len() as f64
        };
        let base = QuantizeOptions { clean: false, triplets: false, robust_tempo: true, ..Default::default() };
        let lax = QuantizeOptions { lax_tightness: Some(4.0), ..base.clone() };
        // tempo constante: no se activa el seguimiento flexible
        let steady = notes_to_score(&rubato_piece(0.0), &lax).unwrap();
        assert!(!steady.warnings.iter().any(|w| w.contains("rubato")), "{:?}", steady.warnings);
        // rubato del 20 %: se activa y las notas caen en la rejilla (el rígido las desplaza)
        let rig = notes_to_score(&rubato_piece(0.2), &base).unwrap();
        let fl = notes_to_score(&rubato_piece(0.2), &lax).unwrap();
        assert!(fl.warnings.iter().any(|w| w.contains("rubato")), "{:?} | rígido en rejilla {:.2}, flexible {:.2}", fl.warnings, on_grid(&rig), on_grid(&fl));
        assert!(on_grid(&fl) >= 0.95 && on_grid(&fl) > on_grid(&rig), "flexible {:.2} rígido {:.2}", on_grid(&fl), on_grid(&rig));
    }
}
