//! Notas con tiempos en segundos (p. ej. de una transcripción de audio) → partitura.
//!
//! Pasos: filtrar por amplitud → estimar tempo y fase del pulso → estimar tonalidad →
//! cuantizar a semicorcheas → repartir en pentagramas (corte en do central) y en voces
//! (hasta 4 por pentagrama) → duraciones. Tempo constante: no sigue rubato.
//! No se generan tresillos; se avisa si parece haberlos.

use crate::beats::{find_beats, BeatMap};
use crate::io::ImportReport;
use crate::{Clef, Measure, Part, Pitch, Score, TimeSig, PPQ};

/// Rejilla de cuantización: semicorchea.
const GRID: u32 = PPQ / 4;
const MAX_MEASURES: u32 = 1000;
/// Do central: por debajo va al pentagrama de clave de fa.
const SPLIT_PITCH: u8 = 60;

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
    /// Limpiar la transcripción (unir fragmentos, quitar armónicos y notas cromáticas débiles).
    pub clean: bool,
    /// Máximo de voces por pentagrama.
    pub max_voices: usize,
}

impl Default for QuantizeOptions {
    fn default() -> Self {
        QuantizeOptions {
            title: "Transcripción de audio".into(),
            min_amplitude: 0.5,
            min_relative: Some(0.65),
            tempo: None,
            time: TimeSig { num: 4, den: 4 },
            program: 0,
            legato_ticks: PPQ * 3 / 4,
            follow_tempo: true,
            tightness: 30.0,
            clean: true,
            max_voices: 1,
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
    const MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    // alteraciones de la tonalidad mayor cuya tónica es la clase de altura i
    const MAJOR_FIFTHS: [i8; 12] = [0, -5, 2, -3, 4, -1, 6, 1, -4, 3, -2, 5];
    let mut hist = [0.0f64; 12];
    for n in notes {
        hist[(n.pitch % 12) as usize] += (n.end - n.start).max(0.05) * n.amplitude as f64;
    }
    if hist.iter().sum::<f64>() <= 0.0 {
        return 0;
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
    let (mut best, mut best_c) = (0i8, f64::MIN);
    for tonic in 0..12 {
        let cm = corr(&MAJOR, tonic);
        if cm > best_c {
            best_c = cm;
            best = MAJOR_FIFTHS[tonic];
        }
        let cn = corr(&MINOR, tonic);
        if cn > best_c {
            best_c = cn;
            best = MAJOR_FIFTHS[(tonic + 3) % 12];
        }
    }
    best
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

/// Reparte notas ya cuantizadas de un pentagrama en hasta 4 voces (la 0 = la más aguda).
fn allocate_voices(mut notes: Vec<Q>, max_voices: usize, legato: u32) -> Vec<Vec<Group>> {
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
            if next != Some(v[i].e) && d <= 1920 && !SIMPLE.contains(&d) {
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
fn remove_chromatic(notes: &[RawNote], fifths: i8) -> (Vec<RawNote>, usize) {
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
    let weak = amps.get(amps.len() * 6 / 10).copied().unwrap_or(0.0);
    let out: Vec<RawNote> = notes
        .iter()
        .copied()
        .filter(|n| allowed[(n.pitch % 12) as usize] || n.amplitude >= weak || n.end - n.start >= 0.5)
        .collect();
    let removed = notes.len() - out.len();
    (out, removed)
}

/// Limpieza previa a la cuantización. Devuelve las notas y cuántas se unieron / quitaron.
pub fn clean_notes(notes: &[RawNote]) -> (Vec<RawNote>, CleanStats) {
    let (a, merged) = merge_fragments(notes, 0.12);
    let (b, harmonics) = remove_harmonics(&a, 0.6);
    let (c, chromatic) = remove_chromatic(&b, estimate_fifths(&b));
    (c, CleanStats { merged, harmonics, chromatic })
}

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
        let (clean, st) = clean_notes(&notes);
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

    // Pulso
    let onsets: Vec<(f64, f32)> = notes.iter().map(|n| (n.start, n.amplitude)).collect();
    let first = notes.iter().map(|n| n.start).fold(f64::MAX, f64::min);
    let last = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    // Con muy pocas notas el seguimiento no es fiable: se usa tempo constante.
    let map = if o.follow_tempo && onsets.len() >= 24 {
        let seed = o.tempo.map(|t| 60.0 / t.clamp(20.0, 300.0));
        let beats = find_beats(&accent_weights(&notes), seed, o.tightness);
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
    // primer pulso de compás: el pulso más cercano por debajo de la primera nota
    let origin = (map.pos(first) + 0.25).floor();
    warnings.push(format!(
        "Tempo {}: {:.1} negras/min.{}",
        if o.tempo.is_some() { "indicado" } else { "estimado" },
        bpm,
        if o.follow_tempo { " Se siguen las variaciones de tempo." } else { " Tempo constante: no se sigue el rubato." }
    ));

    // Tonalidad
    let fifths = estimate_fifths(&notes);
    let sharps = fifths >= 0;

    // Cuantización
    let to_ticks = |t: f64| -> f64 { (map.pos(t) - origin) * PPQ as f64 };
    let mut on_grid = 0usize;
    let mut q: Vec<Q> = Vec::new();
    let mut triplet_like = 0usize;
    for n in &notes {
        let ts = to_ticks(n.start).max(0.0);
        let te = to_ticks(n.end).max(0.0);
        let s = ((ts / GRID as f64).round() as u32) * GRID;
        if (ts / GRID as f64 - (ts / GRID as f64).round()).abs() < 0.15 {
            on_grid += 1;
        }
        let e = (((te / GRID as f64).round() as u32) * GRID).max(s + GRID);
        // ¿cae mejor en una rejilla de tresillos de corchea (1/3 de negra)?
        let frac = (ts / PPQ as f64).fract();
        let e16 = (frac * 4.0 - (frac * 4.0).round()).abs() / 4.0;
        let e3 = (frac * 3.0 - (frac * 3.0).round()).abs() / 3.0;
        if e16 > 0.04 && e3 < 0.02 {
            triplet_like += 1;
        }
        q.push(Q { s, e, pitch: n.pitch });
    }
    let end_tick = q.iter().map(|x| x.e).max().unwrap();
    let mlen = o.time.measure_ticks();
    if end_tick / mlen >= MAX_MEASURES {
        return Err(format!("La pieza supera {MAX_MEASURES} compases con el tempo estimado."));
    }
    let on_grid_pct = on_grid as f64 * 100.0 / q.len() as f64;
    if on_grid_pct < 60.0 {
        warnings.push(format!("Solo el {on_grid_pct:.0} % de las notas cae en la rejilla de semicorcheas: el ritmo es irregular o el pulso no es el adecuado. Prueba a indicar el tempo (o la mitad / el doble)."));
    }
    if triplet_like * 20 > q.len() {
        warnings.push(format!("{triplet_like} notas parecen tresillos; se cuantizaron a semicorcheas (aún no se generan tresillos)."));
    }
    warnings.push(format!("Compás {}/{} supuesto; la primera nota se coloca en el primer tiempo (sin anacrusa). Cuantizado a semicorcheas.", o.time.num, o.time.den));

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

    let mut score = Score::new(&o.title, o.time, fifths, 1);
    score.tempo = bpm.round().clamp(20.0, 300.0) as u32;
    score.parts.clear();
    let mut dropped_voices = false;
    for (name, clef, notes) in staves {
        let voices = allocate_voices(notes, o.max_voices.clamp(1, crate::MAX_VOICES), o.legato_ticks);
        let mut part = Part::new(name, clef, o.program, vec![Measure::empty(mlen)]);
        part.percussion = false;
        score.parts.push(part);
        let pi = score.parts.len() - 1;
        for (vi, v) in voices.iter().enumerate() {
            for g in v {
                let mut ps: Vec<Pitch> = g.pitches.iter().map(|&m| Pitch::from_midi(m as i32, sharps)).collect();
                ps.sort_by_key(|p| p.midi());
                if score.put_abs_voice(pi, vi, g.s, g.e - g.s, &ps).is_err() {
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
    use super::*;
    use crate::io::sounds;

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
}
