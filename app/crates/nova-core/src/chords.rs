//! Detección de acordes a partir de las notas de una partitura: perfil de clases de altura por
//! pulso, comparación con plantillas (tríadas y séptimas) con peso extra del bajo y suavizado de
//! Viterbi (un acorde cambia solo si la ganancia supera `CHANGE_PENALTY`).
use crate::{Content, Score, PPQ};
use serde::{Deserialize, Serialize};

/// Calidad del acorde (el sufijo del cifrado).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Major,
    Minor,
    Dominant7,
    Major7,
    Minor7,
    Dim,
    Dim7,
}

impl Quality {
    pub const ALL: [Quality; 7] = [Quality::Major, Quality::Minor, Quality::Dominant7, Quality::Major7, Quality::Minor7, Quality::Dim, Quality::Dim7];

    /// Intervalos (semitonos sobre la fundamental).
    fn tones(self) -> &'static [u8] {
        match self {
            Quality::Major => &[0, 4, 7],
            Quality::Minor => &[0, 3, 7],
            Quality::Dominant7 => &[0, 4, 7, 10],
            Quality::Major7 => &[0, 4, 7, 11],
            Quality::Minor7 => &[0, 3, 7, 10],
            Quality::Dim => &[0, 3, 6],
            Quality::Dim7 => &[0, 3, 6, 9],
        }
    }

    /// Sufijo del cifrado.
    pub fn suffix(self) -> &'static str {
        match self {
            Quality::Major => "",
            Quality::Minor => "m",
            Quality::Dominant7 => "7",
            Quality::Major7 => "maj7",
            Quality::Minor7 => "m7",
            Quality::Dim => "dim",
            Quality::Dim7 => "dim7",
        }
    }

    /// La tríada de base: 0 mayor, 1 menor, 2 disminuida (para comparar sin las séptimas).
    pub fn triad(self) -> u8 {
        match self {
            Quality::Major | Quality::Dominant7 | Quality::Major7 => 0,
            Quality::Minor | Quality::Minor7 => 1,
            Quality::Dim | Quality::Dim7 => 2,
        }
    }
}

/// Un acorde que empieza en `(measure, tick)` y dura hasta el siguiente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordMark {
    pub measure: usize,
    pub tick: u32,
    /// Clase de altura de la fundamental (0 = do).
    pub root: u8,
    pub quality: Quality,
    /// Clase de altura del bajo (igual a `root` si no hay inversión).
    pub bass: u8,
}

impl ChordMark {
    /// Cifrado, con sostenidos o bemoles según la tonalidad (`fifths`).
    pub fn name(&self, fifths: i8) -> String {
        const SHARP: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
        const FLAT: [&str; 12] = ["C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B"];
        let n = if fifths >= 0 { &SHARP } else { &FLAT };
        let mut s = format!("{}{}", n[self.root as usize % 12], self.quality.suffix());
        if self.bass != self.root {
            s.push('/');
            s.push_str(n[self.bass as usize % 12]);
        }
        s
    }
}

/// Coste de cambiar de acorde entre dos pulsos consecutivos (en unidades de la puntuación de observación).
pub const CHANGE_PENALTY: f64 = 0.25;
/// Penalización a priori de las séptimas frente a las tríadas (solo se eligen con evidencia).
const SEVENTH_PENALTY: f64 = 0.08;
/// Penalización por cada nota de la plantilla que no suena en el pulso.
const MISSING_TONE: f64 = 0.15;
/// Peso extra de las notas graves (por debajo de MIDI 55), donde suele estar la fundamental.
const BASS_WEIGHT: f64 = 1.5;
const BASS_BELOW: i32 = 55;

/// Nota para el análisis: inicio y fin absolutos (ticks) y altura MIDI.
#[derive(Debug, Clone, Copy)]
pub struct CNote {
    pub start: u32,
    pub end: u32,
    pub midi: i32,
}

/// Notas de todas las voces y pentagramas, en el tiempo escrito (sin desplegar repeticiones).
pub fn score_notes(score: &Score) -> Vec<CNote> {
    let n = score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    let starts = score.measure_starts(n);
    let mut out = Vec::new();
    for part in score.parts.iter().filter(|p| !p.percussion) {
        for (mi, m) in part.measures.iter().enumerate() {
            for v in 0..crate::MAX_VOICES {
                if let Some(evs) = m.voice(v) {
                    for e in evs {
                        if let Content::Note { pitches, .. } = &e.content {
                            for p in pitches {
                                out.push(CNote { start: starts[mi] + e.start, end: starts[mi] + e.start + e.ticks, midi: p.midi() });
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Duración de un pulso (ticks) del compás: la negra, o la negra con puntillo en compuesto.
fn beat_ticks(num: u32, den: u32) -> u32 {
    if den == 8 && num % 3 == 0 && num >= 6 {
        PPQ * 3 / 2
    } else {
        (PPQ * 4 / den).max(1)
    }
}

impl ChordMark {
    /// Lee un nombre de acorde escrito a mano ("Am7", "F♯dim", "Bb/D", "CM7", "Dm", "G7"…).
    /// Devuelve (fundamental, calidad, bajo) o `None` si no se entiende.
    pub fn parse(text: &str) -> Option<(u8, Quality, u8)> {
        fn note(s: &str) -> Option<(u8, &str)> {
            let mut ch = s.chars();
            let base = match ch.next()?.to_ascii_uppercase() {
                'C' => 0,
                'D' => 2,
                'E' => 4,
                'F' => 5,
                'G' => 7,
                'A' => 9,
                'B' => 11,
                _ => return None,
            };
            let rest = ch.as_str();
            let (alt, rest) = match rest.chars().next() {
                Some('#') | Some('♯') => (1, &rest[rest.chars().next()?.len_utf8()..]),
                Some('b') | Some('♭') => (-1, &rest[rest.chars().next()?.len_utf8()..]),
                _ => (0, rest),
            };
            Some((((base + alt + 12) % 12) as u8, rest))
        }
        let text = text.trim();
        let (body, bass_txt) = match text.split_once('/') {
            Some((a, b)) => (a, Some(b)),
            None => (text, None),
        };
        let (root, suffix) = note(body)?;
        let quality = match suffix.trim() {
            "" | "maj" | "M" | "major" => Quality::Major,
            "m" | "min" | "mi" | "-" | "minor" => Quality::Minor,
            "7" | "dom7" => Quality::Dominant7,
            "maj7" | "Maj7" | "M7" | "Δ" | "Δ7" | "△" | "△7" => Quality::Major7,
            "m7" | "min7" | "mi7" | "-7" => Quality::Minor7,
            "dim" | "°" | "o" => Quality::Dim,
            "dim7" | "°7" | "o7" => Quality::Dim7,
            _ => return None,
        };
        let bass = match bass_txt {
            Some(b) => {
                let (n, rest) = note(b.trim())?;
                if !rest.is_empty() {
                    return None;
                }
                n
            }
            None => root,
        };
        Some((root, quality, bass))
    }
}

impl Score {
    /// Fija el acorde que empieza en `(measure, tick)`: lo sustituye si ya hay uno, lo inserta si no,
    /// y con texto vacío lo borra. Devuelve error si el nombre no se entiende o el compás no existe.
    pub fn set_chord(&mut self, measure: usize, tick: u32, text: &str) -> Result<(), String> {
        let nmeasures = self.parts.first().map_or(0, |p| p.measures.len());
        if measure >= nmeasures {
            return Err("ese compás no existe".into());
        }
        if text.trim().is_empty() {
            self.chords.retain(|c| (c.measure, c.tick) != (measure, tick));
            return Ok(());
        }
        let (root, quality, bass) = ChordMark::parse(text).ok_or_else(|| format!("no entiendo el acorde «{}» (ejemplos: C, Am, G7, Fmaj7, Dm7, B°, C/E)", text.trim()))?;
        self.chords.retain(|c| (c.measure, c.tick) != (measure, tick));
        self.chords.push(ChordMark { measure, tick, root, quality, bass });
        self.chords.sort_by_key(|c| (c.measure, c.tick));
        Ok(())
    }

    /// Sustituye el cifrado de la partitura por el detectado en sus notas.
    pub fn detect_chord_symbols(&mut self) {
        self.chords = detect_chords(self);
    }
}

/// Acordes de la partitura (uno por cambio), analizando por pulsos. Vacío si no hay notas.
pub fn detect_chords(score: &Score) -> Vec<ChordMark> {
    let notes = score_notes(score);
    let n = score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    let starts = score.measure_starts(n);
    // segmentos: (compás, tick dentro del compás, inicio absoluto, fin absoluto)
    let mut segs: Vec<(usize, u32, u32, u32)> = Vec::new();
    for m in 0..n {
        let len = score.measure_len(m);
        let ts = score.time_at(m);
        let bl = beat_ticks(ts.num, ts.den);
        let mut t = 0;
        while t < len {
            let e = (t + bl).min(len);
            segs.push((m, t, starts[m] + t, starts[m] + e));
            t = e;
        }
    }
    detect_in_segments(&notes, &segs)
}

fn detect_in_segments(notes: &[CNote], segs: &[(usize, u32, u32, u32)]) -> Vec<ChordMark> {
    if segs.is_empty() {
        return Vec::new();
    }
    // observación por segmento: puntuación de cada (fundamental, calidad)
    let nstates = 12 * Quality::ALL.len();
    let mut obs: Vec<Option<Vec<f64>>> = Vec::with_capacity(segs.len());
    let mut basses: Vec<Option<u8>> = Vec::with_capacity(segs.len());
    for &(_, _, a, b) in segs {
        let bl = (b - a).max(1) as f64;
        let mut prof = [0.0f64; 12];
        let mut low: Option<(i32, f64)> = None; // nota más grave con presencia suficiente
        for n in notes.iter().filter(|n| n.start < b && n.end > a) {
            let ov = (n.end.min(b) - n.start.max(a)) as f64;
            let mut w = ov / bl;
            if n.start >= a {
                w *= 1.5; // el ataque dentro del pulso cuenta más que una nota que viene sostenida
            }
            if n.midi < BASS_BELOW {
                w *= BASS_WEIGHT;
            }
            prof[n.midi.rem_euclid(12) as usize] += w;
            if ov / bl >= 0.25 && low.map_or(true, |(m, _)| n.midi < m) {
                low = Some((n.midi, ov));
            }
        }
        let total: f64 = prof.iter().sum();
        if total < 0.2 {
            obs.push(None);
            basses.push(None);
            continue;
        }
        let bass = low.map(|(m, _)| m.rem_euclid(12) as u8);
        let mut sc = vec![0.0; nstates];
        for root in 0..12usize {
            for (qi, q) in Quality::ALL.iter().enumerate() {
                let tones = q.tones();
                let (mut inside, mut missing) = (0.0, 0.0);
                for &t in tones {
                    let v = prof[(root + t as usize) % 12];
                    inside += v;
                    if v < 0.05 * total {
                        missing += 1.0;
                    }
                }
                let outside = total - inside;
                let mut s = (inside - 0.8 * outside) / total - MISSING_TONE * missing;
                if tones.len() == 4 {
                    s -= SEVENTH_PENALTY;
                }
                if let Some(b) = bass {
                    let rel = (b as usize + 12 - root) % 12;
                    if rel == 0 {
                        s += 0.3;
                    } else if tones.iter().any(|&t| t as usize == rel) {
                        s += 0.1;
                    }
                }
                sc[root * Quality::ALL.len() + qi] = s;
            }
        }
        obs.push(Some(sc));
        basses.push(bass);
    }
    if obs.iter().all(|o| o.is_none()) {
        return Vec::new();
    }
    // Viterbi: maximizar la suma de observaciones menos CHANGE_PENALTY por cambio
    let mut best = vec![0.0f64; nstates];
    let mut back: Vec<Vec<u16>> = Vec::with_capacity(segs.len());
    for (i, o) in obs.iter().enumerate() {
        let (top_i, top_v) = best.iter().cloned().enumerate().fold((0, f64::MIN), |a, (j, v)| if v > a.1 { (j, v) } else { a });
        let mut next = vec![0.0; nstates];
        let mut bp = vec![0u16; nstates];
        for s in 0..nstates {
            let (from, base) = if i == 0 || best[s] >= top_v - CHANGE_PENALTY { (s, best[s]) } else { (top_i, top_v - CHANGE_PENALTY) };
            bp[s] = from as u16;
            next[s] = base + o.as_ref().map_or(0.0, |v| v[s]);
        }
        best = next;
        back.push(bp);
    }
    let mut state = best.iter().cloned().enumerate().fold((0, f64::MIN), |a, (j, v)| if v > a.1 { (j, v) } else { a }).0;
    let mut path = vec![0usize; segs.len()];
    for i in (0..segs.len()).rev() {
        path[i] = state;
        state = back[i][state] as usize;
    }
    // marcas: solo donde cambia el acorde (a partir del primer segmento con notas)
    let first = obs.iter().position(|o| o.is_some()).unwrap();
    let mut out: Vec<ChordMark> = Vec::new();
    for i in first..segs.len() {
        if i == first || path[i] != path[i - 1] {
            let root = (path[i] / Quality::ALL.len()) as u8;
            let quality = Quality::ALL[path[i] % Quality::ALL.len()];
            // bajo del primer segmento donde suena este acorde
            let bass = basses[i].unwrap_or(root);
            out.push(ChordMark { measure: segs[i].0, tick: segs[i].1, root, quality, bass });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hand_written_chord_names() {
        assert_eq!(ChordMark::parse("C"), Some((0, Quality::Major, 0)));
        assert_eq!(ChordMark::parse("Am"), Some((9, Quality::Minor, 9)));
        assert_eq!(ChordMark::parse("F♯dim7"), Some((6, Quality::Dim7, 6)));
        assert_eq!(ChordMark::parse("Bb/D"), Some((10, Quality::Major, 2)));
        assert_eq!(ChordMark::parse("Bdim"), Some((11, Quality::Dim, 11)));
        assert_eq!(ChordMark::parse("EbM7"), Some((3, Quality::Major7, 3)));
        assert_eq!(ChordMark::parse(" g7 "), Some((7, Quality::Dominant7, 7)));
        assert_eq!(ChordMark::parse("Dm7/C"), Some((2, Quality::Minor7, 0)));
        assert_eq!(ChordMark::parse("Csus4"), None);
        assert_eq!(ChordMark::parse("H"), None);
        assert_eq!(ChordMark::parse("C/"), None);
    }

    #[test]
    fn set_chord_replaces_inserts_and_deletes() {
        let mut s = Score::new("t", crate::TimeSig { num: 4, den: 4 }, 0, 2);
        s.set_chord(1, 0, "G7").unwrap();
        s.set_chord(0, 0, "C").unwrap();
        s.set_chord(1, 0, "Am").unwrap();
        assert_eq!(s.chords.iter().map(|c| c.name(0)).collect::<Vec<_>>(), vec!["C", "Am"]);
        assert!(s.set_chord(0, 0, "Xyz").is_err());
        assert!(s.set_chord(9, 0, "C").is_err());
        s.set_chord(1, 0, "").unwrap();
        assert_eq!(s.chords.len(), 1);
    }

    fn seg_grid(beats: u32) -> Vec<(usize, u32, u32, u32)> {
        (0..beats).map(|b| ((b / 4) as usize, (b % 4) * PPQ, b * PPQ, (b + 1) * PPQ)).collect()
    }

    fn notes(chords: &[(i32, &[i32])]) -> Vec<CNote> {
        let mut v = Vec::new();
        for (i, (bass, tones)) in chords.iter().enumerate() {
            let (a, b) = (i as u32 * 4 * PPQ, (i as u32 + 1) * 4 * PPQ);
            v.push(CNote { start: a, end: b, midi: *bass });
            for t in tones.iter() {
                v.push(CNote { start: a, end: b, midi: *t });
            }
        }
        v
    }

    #[test]
    fn detects_simple_progression_with_qualities() {
        // C (do-mi-sol), Am, G7, C: un acorde por compás
        let n = notes(&[(36, &[60, 64, 67]), (33, &[60, 64, 69]), (31, &[59, 62, 65, 67]), (36, &[60, 64, 67])]);
        let m = detect_in_segments(&n, &seg_grid(16));
        let got: Vec<(u8, Quality)> = m.iter().map(|c| (c.root, c.quality)).collect();
        assert_eq!(got, vec![(0, Quality::Major), (9, Quality::Minor), (7, Quality::Dominant7), (0, Quality::Major)]);
        assert_eq!(m[2].measure, 2);
    }

    #[test]
    fn passing_tones_do_not_change_the_chord() {
        // acorde de do mayor con una nota de paso corta en el pulso 2: no debe partir el compás
        let mut n = notes(&[(36, &[60, 64, 67])]);
        n.push(CNote { start: PPQ, end: PPQ + PPQ / 4, midi: 62 });
        let m = detect_in_segments(&n, &seg_grid(4));
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].root, m[0].quality), (0, Quality::Major));
    }

    #[test]
    fn chords_survive_json_and_old_files_without_them_open() {
        let mut sc = Score::new("t", crate::TimeSig { num: 4, den: 4 }, 0, 2);
        sc.chords = vec![ChordMark { measure: 1, tick: 480, root: 9, quality: Quality::Minor7, bass: 4 }];
        let json = serde_json::to_string(&sc).unwrap();
        assert!(json.contains("\"minor7\""));
        let back: Score = serde_json::from_str(&json).unwrap();
        assert_eq!(back.chords, sc.chords);
        // un .nova antiguo no tiene el campo
        sc.chords.clear();
        let old = serde_json::to_string(&sc).unwrap();
        assert!(!old.contains("chords"));
        assert!(serde_json::from_str::<Score>(&old).unwrap().chords.is_empty());
    }

    #[test]
    fn chords_round_trip_through_musicxml() {
        use crate::Pitch;
        for fifths in [0i8, -3] {
            let mut sc = Score::new("t", crate::TimeSig { num: 4, den: 4 }, fifths, 3);
            for m in 0..3 {
                sc.put_note(0, 0, m, 0, PPQ * 4, vec![Pitch::from_midi(60, true)]).unwrap();
            }
            sc.chords = vec![
                ChordMark { measure: 0, tick: 0, root: 0, quality: Quality::Major, bass: 0 },
                ChordMark { measure: 0, tick: PPQ * 2, root: 9, quality: Quality::Minor7, bass: 4 }, // a mitad de una nota larga: desplazamiento
                ChordMark { measure: 1, tick: 0, root: 3, quality: Quality::Dominant7, bass: 3 }, // Mi♭7 / Re♯7 según la tonalidad
                ChordMark { measure: 2, tick: PPQ, root: 6, quality: Quality::Dim7, bass: 6 },
            ];
            let xml = crate::io::export_musicxml(&sc);
            assert!(xml.contains("<harmony>") && xml.contains("<offset>960</offset>"));
            let back = crate::io::import_musicxml(&xml).unwrap().score;
            assert_eq!(back.chords, sc.chords, "tonalidad {fifths}");
        }
    }

    #[test]
    fn unknown_harmony_kinds_are_skipped_with_a_warning() {
        let mut sc = Score::new("t", crate::TimeSig { num: 4, den: 4 }, 0, 1);
        sc.put_note(0, 0, 0, 0, PPQ * 4, vec![crate::Pitch::from_midi(60, true)]).unwrap();
        sc.chords = vec![ChordMark { measure: 0, tick: 0, root: 0, quality: Quality::Major, bass: 0 }];
        let xml = crate::io::export_musicxml(&sc).replace("<kind text=\"\">major</kind>", "<kind>suspended-fourth</kind>");
        let r = crate::io::import_musicxml(&xml).unwrap();
        assert!(r.score.chords.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("no se importaron")));
    }

    #[test]
    fn inversion_is_reported_as_bass() {
        let n = notes(&[(40, &[60, 67, 72])]); // do mayor con mi en el bajo
        let m = detect_in_segments(&n, &seg_grid(4));
        assert_eq!((m[0].root, m[0].bass), (0, 4));
        assert_eq!(m[0].name(0), "C/E");
    }
}
