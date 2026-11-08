//! Port a Rust del post-procesado de Basic Pitch (`output_to_notes_polyphonic`,
//! Spotify, Apache-2.0): convierte las matrices de activación en notas.
//! Sin dependencias: se puede probar sin el modelo.

use crate::{FFT_HOP, MODEL_FRAMES_PER_WINDOW, N_PITCHES, OVERLAP_FRAMES, SAMPLE_RATE, WINDOW_SAMPLES};

/// MIDI de la tecla más grave del modelo (A0).
pub const MIDI_OFFSET: i32 = 21;
const ENERGY_TOL: i64 = 11;

/// Nota detectada. Tiempos en segundos.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEvent {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    /// Amplitud media del modelo, 0..1.
    pub amplitude: f32,
    /// `true` si la nota nace de un inicio claro del modelo; `false` si es una continuación
    /// hallada por el truco melodia (a menudo un fragmento de una nota ya existente).
    pub onset: bool,
}

#[derive(Debug, Clone)]
pub struct Params {
    pub onset_threshold: f32,
    pub frame_threshold: f32,
    /// Duración mínima de nota en milisegundos.
    pub min_note_ms: f32,
    pub infer_onsets: bool,
    pub melodia: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { onset_threshold: 0.5, frame_threshold: 0.3, min_note_ms: 127.7, infer_onsets: true, melodia: true }
    }
}

/// Matriz (tiempo × 88) en orden de filas.
pub struct Matrix {
    pub rows: usize,
    pub data: Vec<f32>,
}

impl Matrix {
    pub fn new(rows: usize) -> Self {
        Matrix { rows, data: vec![0.0; rows * N_PITCHES] }
    }
    #[inline]
    pub fn get(&self, t: usize, f: usize) -> f32 {
        self.data[t * N_PITCHES + f]
    }
    #[inline]
    fn set(&mut self, t: usize, f: usize, v: f32) {
        self.data[t * N_PITCHES + f] = v;
    }
}

fn inferred_onsets(onsets: &Matrix, frames: &Matrix) -> Matrix {
    let n = frames.rows;
    let mut diff = Matrix::new(n);
    for t in 0..n {
        for f in 0..N_PITCHES {
            let cur = frames.get(t, f);
            let d1 = cur - if t >= 1 { frames.get(t - 1, f) } else { 0.0 };
            let d2 = cur - if t >= 2 { frames.get(t - 2, f) } else { 0.0 };
            diff.set(t, f, d1.min(d2).max(0.0));
        }
    }
    for t in 0..2.min(n) {
        for f in 0..N_PITCHES {
            diff.set(t, f, 0.0);
        }
    }
    let max_on = onsets.data.iter().cloned().fold(0.0f32, f32::max);
    let max_diff = diff.data.iter().cloned().fold(0.0f32, f32::max);
    let mut out = Matrix::new(n);
    for i in 0..out.data.len() {
        let d = if max_diff > 0.0 { max_on * diff.data[i] / max_diff } else { 0.0 };
        out.data[i] = onsets.data[i].max(d);
    }
    out
}

/// Notas en fotogramas: (inicio, fin, índice 0..88, amplitud).
pub fn frames_to_notes(frames: &Matrix, onsets_in: &Matrix, p: &Params) -> Vec<(usize, usize, usize, f32, bool)> {
    let n = frames.rows;
    if n < 3 {
        return Vec::new();
    }
    let min_len = (p.min_note_ms / 1000.0 * (SAMPLE_RATE as f32 / FFT_HOP as f32)).round() as i64;
    let thresh = p.frame_threshold;
    let onsets = if p.infer_onsets {
        inferred_onsets(onsets_in, frames)
    } else {
        Matrix { rows: n, data: onsets_in.data.clone() }
    };

    // Picos de inicio (máximos locales en el tiempo) por encima del umbral;
    // se recorren hacia atrás en el tiempo y en la frecuencia, igual que el original.
    let mut peaks: Vec<(usize, usize)> = Vec::new();
    for t in 1..n - 1 {
        for f in 0..N_PITCHES {
            let v = onsets.get(t, f);
            if v > onsets.get(t - 1, f) && v > onsets.get(t + 1, f) && v >= p.onset_threshold {
                peaks.push((t, f));
            }
        }
    }
    peaks.reverse();

    let mut rem = Matrix { rows: n, data: frames.data.clone() };
    fn clear(rem: &mut Matrix, a: usize, b: usize, f: usize) {
        for t in a..b {
            rem.set(t, f, 0.0);
            if f + 1 < N_PITCHES {
                rem.set(t, f + 1, 0.0);
            }
            if f > 0 {
                rem.set(t, f - 1, 0.0);
            }
        }
    }
    let mean = |a: usize, b: usize, f: usize| -> f32 { (a..b).map(|t| frames.get(t, f)).sum::<f32>() / (b - a) as f32 };

    let mut notes = Vec::new();
    for (start, f) in peaks {
        if start >= n - 1 {
            continue;
        }
        let mut i = start + 1;
        let mut k = 0i64;
        while i < n - 1 && k < ENERGY_TOL {
            if rem.get(i, f) < thresh {
                k += 1
            } else {
                k = 0
            }
            i += 1;
        }
        let end = (i as i64 - k) as usize;
        if (end as i64 - start as i64) <= min_len {
            continue;
        }
        clear(&mut rem, start, end, f);
        // ¿El modelo predice de verdad un inicio aquí (no solo inferido de cambios de amplitud)?
        let strong = (start.saturating_sub(1)..=(start + 1).min(n - 1)).any(|t| onsets_in.get(t, f) >= p.onset_threshold);
        notes.push((start, end, f, mean(start, end, f), strong));
    }

    if p.melodia {
        loop {
            // Primer máximo en orden de filas (como np.argmax).
            let (mut best, mut bi) = (f32::MIN, 0usize);
            for (idx, &v) in rem.data.iter().enumerate() {
                if v > best {
                    best = v;
                    bi = idx;
                }
            }
            if best <= thresh {
                break;
            }
            let (mid, f) = (bi / N_PITCHES, bi % N_PITCHES);
            rem.set(mid, f, 0.0);
            // hacia delante
            let mut i = mid + 1;
            let mut k = 0i64;
            while i < n - 1 && k < ENERGY_TOL {
                if rem.get(i, f) < thresh {
                    k += 1
                } else {
                    k = 0
                }
                clear(&mut rem, i, i + 1, f);
                i += 1;
            }
            let i_end = i as i64 - 1 - k;
            // hacia atrás
            let mut i = mid as i64 - 1;
            let mut k = 0i64;
            while i > 0 && k < ENERGY_TOL {
                if rem.get(i as usize, f) < thresh {
                    k += 1
                } else {
                    k = 0
                }
                clear(&mut rem, i as usize, i as usize + 1, f);
                i -= 1;
            }
            let i_start = i + 1 + k;
            if i_end - i_start <= min_len {
                continue;
            }
            notes.push((i_start as usize, i_end as usize, f, mean(i_start as usize, i_end as usize, f), false));
        }
    }
    notes
}

/// Tiempo (s) de un fotograma tras unir las ventanas recortadas.
/// Cada ventana aporta 142 fotogramas útiles y avanza `WINDOW_SAMPLES - solapamiento` muestras.
pub fn frame_time(frame: usize) -> f64 {
    let useful = MODEL_FRAMES_PER_WINDOW - OVERLAP_FRAMES;
    let hop = WINDOW_SAMPLES - OVERLAP_FRAMES * FFT_HOP;
    let w = frame / useful;
    let j = frame % useful;
    (w * hop + j * FFT_HOP) as f64 / SAMPLE_RATE as f64
}

pub fn to_events(frame_notes: &[(usize, usize, usize, f32, bool)]) -> Vec<NoteEvent> {
    let mut v: Vec<NoteEvent> = frame_notes
        .iter()
        .map(|&(s, e, f, a, on)| NoteEvent {
            start: frame_time(s),
            end: frame_time(e),
            pitch: (f as i32 + MIDI_OFFSET) as u8,
            amplitude: a,
            onset: on,
        })
        .collect();
    v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap().then(a.pitch.cmp(&b.pitch)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(frames: usize, notes: &[(usize, usize, usize)]) -> (Matrix, Matrix) {
        let mut fr = Matrix::new(frames);
        let mut on = Matrix::new(frames);
        for &(s, e, f) in notes {
            for t in s..e {
                fr.set(t, f, 0.9);
            }
            on.set(s, f, 0.9);
        }
        (fr, on)
    }

    #[test]
    fn detects_two_notes() {
        let (fr, on) = synth(200, &[(10, 60, 48), (80, 150, 52)]);
        let mut n = frames_to_notes(&fr, &on, &Params::default());
        n.sort_by_key(|x| (x.0, x.2));
        assert_eq!(n.len(), 2);
        assert_eq!((n[0].0, n[0].2), (10, 48));
        assert!((n[0].1 as i64 - 60).abs() <= 1);
        assert_eq!((n[1].0, n[1].2), (80, 52));
    }

    #[test]
    fn drops_short_notes() {
        let (fr, on) = synth(100, &[(10, 15, 40)]);
        assert!(frames_to_notes(&fr, &on, &Params::default()).is_empty());
    }

    #[test]
    fn melodia_recovers_note_without_onset() {
        let (fr, mut on) = synth(120, &[(10, 80, 60)]);
        on.set(10, 60, 0.0);
        let p = Params { infer_onsets: false, ..Params::default() };
        assert_eq!(frames_to_notes(&fr, &on, &p).len(), 1);
        let q = Params { infer_onsets: false, melodia: false, ..Params::default() };
        assert!(frames_to_notes(&fr, &on, &q).is_empty());
    }

    #[test]
    fn time_mapping_is_monotonic() {
        assert_eq!(frame_time(0), 0.0);
        assert!(frame_time(142) > frame_time(141));
    }
}
