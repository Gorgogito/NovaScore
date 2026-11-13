//! Asistencia a la composición con reglas simbólicas (sin modelos ni descargas).
//!
//! - [`Score::add_accompaniment`] escribe una parte nueva (bajo, acordes o arpegio) a partir del cifrado de acordes.
//! - [`Score::review`] revisa la partitura y lista lo que suele ser un error: notas fuera de la tesitura del
//!   instrumento, voces cruzadas y quintas u octavas paralelas entre dos líneas.

use crate::chords::{ChordMark, Quality};
use crate::*;

/// Estilos de acompañamiento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accompaniment {
    /// La fundamental de cada acorde en el registro grave, una vez por compás.
    Bass,
    /// El acorde completo (tríada o cuatriada) sostenido, una vez por compás.
    Blocks,
    /// Arpegio en corcheas que sube y baja por las notas del acorde.
    Arpeggio,
}

impl Accompaniment {
    fn part_name(self) -> &'static str {
        match self {
            Accompaniment::Bass => "Bajo",
            Accompaniment::Blocks => "Acompañamiento (acordes)",
            Accompaniment::Arpeggio => "Acompañamiento (arpegio)",
        }
    }
}

fn tones(q: Quality) -> &'static [u8] {
    match q {
        Quality::Major => &[0, 4, 7],
        Quality::Minor => &[0, 3, 7],
        Quality::Dominant7 => &[0, 4, 7, 10],
        Quality::Major7 => &[0, 4, 7, 11],
        Quality::Minor7 => &[0, 3, 7, 10],
        Quality::Dim => &[0, 3, 6],
        Quality::Dim7 => &[0, 3, 6, 9],
    }
}

/// Notas (MIDI) del acorde con la fundamental en `[low, low + 12)` y el resto por encima; con bajo invertido
/// el bajo va debajo de todo.
fn chord_notes(c: &ChordMark, low: i32) -> Vec<i32> {
    let root = low + (c.root as i32 - low).rem_euclid(12);
    tones(c.quality).iter().map(|&t| root + t as i32).collect()
}

/// Un acorde y el tramo que dura: de su inicio al siguiente acorde (o al final de la pieza).
struct Span {
    chord: ChordMark,
    from: u32,
    to: u32,
}

impl Score {
    fn chord_spans(&self) -> Vec<Span> {
        let total_measures = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        let starts = self.measure_starts(total_measures + 1);
        let end = *starts.last().unwrap_or(&0);
        let mut chords = self.chords.clone();
        chords.sort_by_key(|c| (c.measure, c.tick));
        let abs = |c: &ChordMark| starts.get(c.measure).copied().unwrap_or(end) + c.tick;
        chords
            .iter()
            .enumerate()
            .map(|(i, c)| Span { chord: *c, from: abs(c), to: chords.get(i + 1).map_or(end, |n| abs(n)).min(end) })
            .filter(|s| s.to > s.from)
            .collect()
    }

    /// Añade una parte de acompañamiento siguiendo el cifrado de acordes (si no hay, se detecta primero).
    /// Devuelve el índice de la parte nueva.
    pub fn add_accompaniment(&mut self, style: Accompaniment) -> Result<usize, String> {
        if self.chords.is_empty() {
            self.detect_chord_symbols();
        }
        let spans = self.chord_spans();
        if spans.is_empty() {
            return Err("No hay acordes: escribe el cifrado (tecla K) o pulsa «Detectar acordes» antes de pedir un acompañamiento.".into());
        }
        let program = if style == Accompaniment::Bass { 32 } else { 0 };
        let pi = self.add_part(style.part_name(), Clef::Bass, program, false);
        let sharps = self.fifths >= 0;
        let note = |m: i32| Pitch::from_midi(m, sharps);
        let mut written = 0usize;
        for sp in &spans {
            // one piece per measure, so long chords are retriggered at every barline
            let mut at = sp.from;
            while at < sp.to {
                let (m, off) = self.locate(at);
                let bar_end = at - off + self.measure_len(m);
                let to = sp.to.min(bar_end);
                let ok = match style {
                    Accompaniment::Bass => {
                        let n = chord_notes(&sp.chord, 36)[0];
                        let bass = if sp.chord.bass != sp.chord.root { 36 + (sp.chord.bass as i32 - 36).rem_euclid(12) } else { n };
                        self.put_abs(pi, at, to - at, &[note(bass)])
                    }
                    Accompaniment::Blocks => {
                        let mut ns = chord_notes(&sp.chord, 48);
                        if sp.chord.bass != sp.chord.root {
                            ns.insert(0, 36 + (sp.chord.bass as i32 - 36).rem_euclid(12));
                        }
                        self.put_abs(pi, at, to - at, &ns.iter().map(|&m| note(m)).collect::<Vec<_>>())
                    }
                    Accompaniment::Arpeggio => {
                        let ns = chord_notes(&sp.chord, 43);
                        // up and down: 1 2 3 4 3 2 ... over the chord tones plus the octave of the root
                        let mut cycle: Vec<i32> = ns.clone();
                        cycle.push(ns[0] + 12);
                        let mut order: Vec<i32> = cycle.clone();
                        order.extend(cycle.iter().rev().skip(1).take(cycle.len().saturating_sub(2)));
                        let mut pos = at;
                        let mut k = 0usize;
                        let mut res = Ok(());
                        while pos < to {
                            let len = (to - pos).min(PPQ / 2);
                            res = res.and(self.put_abs(pi, pos, len, &[note(order[k % order.len()])]));
                            pos += len;
                            k += 1;
                        }
                        res
                    }
                };
                if ok.is_ok() {
                    written += 1;
                }
                at = to;
            }
        }
        // the new part is as long as the others
        let max_len = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(1);
        let lens: Vec<u32> = (0..max_len).map(|m| self.measure_len(m)).collect();
        let p = &mut self.parts[pi];
        while p.measures.len() < max_len {
            p.measures.push(Measure::empty(lens[p.measures.len()]));
        }
        if written == 0 {
            self.parts.remove(pi);
            return Err("No se pudo escribir el acompañamiento.".into());
        }
        Ok(pi)
    }
}

/// Algo que revisar en la partitura.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub part: usize,
    pub voice: usize,
    pub measure: usize,
    pub tick: u32,
    /// `range`, `crossing` o `parallel`.
    pub kind: &'static str,
    pub message: String,
}

/// Tesitura (MIDI de la nota más grave y de la más aguda) de un instrumento General MIDI, si la conocemos.
fn range_of(program: u8) -> Option<(i32, i32, &'static str)> {
    Some(match program {
        0..=7 => (21, 108, "piano"),
        24..=31 => (40, 88, "guitarra"),
        32..=39 => (28, 67, "bajo"),
        40 => (55, 103, "violín"),
        41 => (48, 91, "viola"),
        42 => (36, 76, "violonchelo"),
        43 => (28, 67, "contrabajo"),
        56 => (54, 82, "trompeta"),
        57 => (40, 72, "trombón"),
        58 => (28, 58, "tuba"),
        60 => (41, 77, "trompa"),
        64..=67 => (49, 90, "saxofón"),
        68 => (58, 91, "oboe"),
        70 => (34, 75, "fagot"),
        71 => (50, 94, "clarinete"),
        72 => (74, 102, "flautín"),
        73 => (60, 96, "flauta"),
        _ => return None,
    })
}

fn name_of(midi: i32) -> String {
    const N: [&str; 12] = ["do", "do♯", "re", "re♯", "mi", "fa", "fa♯", "sol", "sol♯", "la", "la♯", "si"];
    format!("{}{}", N[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

/// Un evento con notas de una línea.
struct N {
    abs: u32,
    end: u32,
    top: i32,
    bottom: i32,
    measure: usize,
    tick: u32,
}

/// Una línea melódica: las notas de una voz de una parte.
struct Line {
    part: usize,
    voice: usize,
    notes: Vec<N>,
}

impl Line {
    /// El evento que suena en `x` (empieza en o antes de `x` y no ha terminado).
    fn at(&self, x: u32) -> Option<&N> {
        self.notes.iter().find(|n| n.abs <= x && x < n.end)
    }
}

impl Score {
    fn lines(&self) -> Vec<Line> {
        let n = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        let starts = self.measure_starts(n);
        let mut out = Vec::new();
        for (pi, p) in self.parts.iter().enumerate() {
            if p.percussion {
                continue;
            }
            for v in 0..MAX_VOICES {
                let mut notes = Vec::new();
                for (mi, m) in p.measures.iter().enumerate() {
                    for e in m.voice(v).into_iter().flatten() {
                        if let Content::Note { pitches, .. } = &e.content {
                            let top = pitches.iter().map(|q| q.midi()).max().unwrap_or(0);
                            let bottom = pitches.iter().map(|q| q.midi()).min().unwrap_or(0);
                            notes.push(N { abs: starts[mi] + e.start, end: starts[mi] + e.start + e.ticks, top, bottom, measure: mi, tick: e.start });
                        }
                    }
                }
                if !notes.is_empty() {
                    out.push(Line { part: pi, voice: v, notes });
                }
            }
        }
        out
    }

    /// Revisa la partitura: tesitura, voces cruzadas y quintas/octavas paralelas. De cada tipo se muestran pocos
    /// avisos por línea para no ahogar al usuario.
    pub fn review(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let lines = self.lines();
        // 1. range (at sounding pitch)
        for l in &lines {
            let p = &self.parts[l.part];
            let Some((lo, hi, name)) = range_of(p.program) else { continue };
            let bad: Vec<(&N, i32)> = l.notes.iter().filter_map(|n| if n.top > hi { Some((n, n.top)) } else if n.bottom < lo { Some((n, n.bottom)) } else { None }).collect();
            for (n, midi) in bad.iter().take(8) {
                issues.push(Issue { part: l.part, voice: l.voice, measure: n.measure, tick: n.tick, kind: "range", message: format!("{} suena fuera de la tesitura de «{}» ({} llega de {} a {}).", name_of(*midi), p.name, name, name_of(lo), name_of(hi)) });
            }
            if bad.len() > 8 {
                let last = bad[bad.len() - 1].0;
                issues.push(Issue { part: l.part, voice: l.voice, measure: last.measure, tick: last.tick, kind: "range", message: format!("«{}»: {} notas más fuera de tesitura (solo se muestran las 8 primeras).", p.name, bad.len() - 8) });
            }
        }
        // 2. the voices of one part must not cross (voice 1 above voice 2, and so on)
        for pi in 0..self.parts.len() {
            let ls: Vec<&Line> = lines.iter().filter(|l| l.part == pi).collect();
            for a in &ls {
                for b in ls.iter().filter(|b| b.voice > a.voice) {
                    let mut times: Vec<u32> = a.notes.iter().map(|n| n.abs).chain(b.notes.iter().map(|n| n.abs)).collect();
                    times.sort_unstable();
                    times.dedup();
                    let mut reported = 0;
                    for x in times {
                        if let (Some(na), Some(nb)) = (a.at(x), b.at(x)) {
                            if nb.top > na.top {
                                reported += 1;
                                if reported <= 4 {
                                    let at = if na.abs >= nb.abs { na } else { nb };
                                    issues.push(Issue { part: pi, voice: a.voice, measure: at.measure, tick: at.tick, kind: "crossing", message: format!("Las voces {} y {} se cruzan: la voz {} suena más aguda que la {}.", a.voice + 1, b.voice + 1, b.voice + 1, a.voice + 1) });
                                }
                            }
                        }
                    }
                }
            }
        }
        // 3. parallel fifths and octaves between two lines that move together
        for (i, a) in lines.iter().enumerate() {
            for b in lines.iter().skip(i + 1) {
                let mut found = 0;
                let mut prev: Option<(i32, i32)> = None; // tops at the previous common onset
                for na in &a.notes {
                    let Some(nb) = b.notes.iter().find(|n| n.abs == na.abs) else { continue };
                    if let Some((pa, pb)) = prev {
                        let (ia, ib) = ((pa - pb).rem_euclid(12), (na.top - nb.top).rem_euclid(12));
                        let moved = (na.top - pa).signum() * (nb.top - pb).signum() > 0;
                        if moved && ia == ib && (ia == 7 || ia == 0) && (na.top - nb.top).abs() >= 7 {
                            found += 1;
                            if found <= 3 {
                                let what = if ia == 7 { "quintas" } else { "octavas" };
                                issues.push(Issue { part: a.part, voice: a.voice, measure: na.measure, tick: na.tick, kind: "parallel", message: format!("{what} paralelas entre «{}» (voz {}) y «{}» (voz {}).", self.parts[a.part].name, a.voice + 1, self.parts[b.part].name, b.voice + 1) });
                            }
                        }
                    }
                    prev = Some((na.top, nb.top));
                }
            }
        }
        issues.sort_by_key(|i| (i.measure, i.tick, i.part));
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(m: i32) -> Pitch {
        Pitch::from_midi(m, true)
    }

    fn melody_with_chords() -> Score {
        let mut s = Score::new("m", TimeSig { num: 4, den: 4 }, 0, 4);
        for m in 0..4u32 {
            s.put_abs(0, m * 1920, 1920, &[p(72)]).unwrap();
        }
        // C | C | F | G7
        for (m, root, q) in [(0, 0, Quality::Major), (2, 5, Quality::Major), (3, 7, Quality::Dominant7)] {
            s.chords.push(ChordMark { measure: m, tick: 0, root, quality: q, bass: root, manual: true });
        }
        s
    }

    #[test]
    fn bass_follows_the_chords_one_note_per_bar() {
        let mut s = melody_with_chords();
        let pi = s.add_accompaniment(Accompaniment::Bass).unwrap();
        assert_eq!((s.parts[pi].clef, s.parts[pi].program, s.parts[pi].name.as_str()), (Clef::Bass, 32, "Bajo"));
        let roots: Vec<i32> = s.parts[pi].measures.iter().map(|m| if let Content::Note { pitches, .. } = &m.events[0].content { pitches[0].midi() } else { -1 }).collect();
        // C held through bars 1-2 (retriggered), F, G
        assert_eq!(roots, vec![36, 36, 41, 43]);
        assert!(s.parts[pi].measures.iter().all(|m| m.events.len() == 1 && m.events[0].ticks == 1920));
    }

    #[test]
    fn blocks_and_arpeggio_use_the_chord_tones() {
        let mut s = melody_with_chords();
        let pi = s.add_accompaniment(Accompaniment::Blocks).unwrap();
        let chord = |s: &Score, m: usize| -> Vec<i32> { if let Content::Note { pitches, .. } = &s.parts[pi].measures[m].events[0].content { pitches.iter().map(|q| q.midi()).collect() } else { vec![] } };
        assert_eq!(chord(&s, 0), vec![48, 52, 55]); // C E G
        assert_eq!(chord(&s, 2), vec![53, 57, 60]); // F A C
        assert_eq!(chord(&s, 3), vec![55, 59, 62, 65]); // G B D F
        let mut t = melody_with_chords();
        let pa = t.add_accompaniment(Accompaniment::Arpeggio).unwrap();
        let evs = &t.parts[pa].measures[0].events;
        assert_eq!(evs.len(), 8, "eighth notes");
        let m: Vec<i32> = evs.iter().map(|e| if let Content::Note { pitches, .. } = &e.content { pitches[0].midi() } else { -1 }).collect();
        // all notes belong to C major triad (+ octave of the root)
        assert!(m.iter().all(|x| [0, 4, 7].contains(&(x % 12))), "{m:?}");
        assert!(m[0] < m[3] && m[3] > m[6], "up then down: {m:?}");
    }

    #[test]
    fn without_chords_they_are_detected_first_and_an_empty_score_is_refused() {
        let mut s = Score::new("m", TimeSig { num: 4, den: 4 }, 0, 4);
        for m in 0..4u32 {
            for (k, n) in [60, 64, 67, 64].iter().enumerate() {
                s.put_abs(0, m * 1920 + k as u32 * 480, 480, &[p(*n)]).unwrap();
            }
        }
        assert!(s.chords.is_empty());
        assert!(s.add_accompaniment(Accompaniment::Bass).is_ok());
        assert!(!s.chords.is_empty(), "detected on the way");
        let mut empty = Score::new("e", TimeSig { num: 4, den: 4 }, 0, 2);
        assert!(empty.add_accompaniment(Accompaniment::Bass).is_err());
        assert_eq!(empty.parts.len(), 1, "no part is left behind");
    }

    #[test]
    fn review_finds_range_crossing_and_parallels() {
        // a violin with a note below its G3
        let mut s = Score::new("r", TimeSig { num: 4, den: 4 }, 0, 2);
        s.parts[0].program = 40;
        s.put_abs(0, 0, 1920, &[p(60)]).unwrap();
        s.put_abs(0, 1920, 1920, &[p(48)]).unwrap();
        let issues = s.review();
        assert!(issues.iter().any(|i| i.kind == "range" && i.measure == 1 && i.message.contains("violín")), "{issues:?}");
        assert!(!issues.iter().any(|i| i.measure == 0 && i.kind == "range"));
        // two voices that cross: voice 2 above voice 1
        let mut c = Score::new("c", TimeSig { num: 4, den: 4 }, 0, 2);
        c.put_abs_voice(0, 0, 0, 1920, &[p(60)]).unwrap();
        c.put_abs_voice(0, 1, 0, 1920, &[p(67)]).unwrap();
        assert!(c.review().iter().any(|i| i.kind == "crossing"), "{:?}", c.review());
        let mut ok = Score::new("o", TimeSig { num: 4, den: 4 }, 0, 2);
        ok.put_abs_voice(0, 0, 0, 1920, &[p(72)]).unwrap();
        ok.put_abs_voice(0, 1, 0, 1920, &[p(60)]).unwrap();
        assert!(ok.review().iter().all(|i| i.kind != "crossing"));
        // parallel fifths between two parts: C-G, D-A move together
        let mut f = Score::new("f", TimeSig { num: 4, den: 4 }, 0, 2);
        let b = f.add_part("Bajo", Clef::Bass, 0, false);
        for (k, (hi, lo)) in [(79, 72 - 12 - 12 + 0), (81, 74 - 24)].iter().enumerate() {
            f.put_abs(0, k as u32 * 480, 480, &[p(*hi)]).unwrap();
            f.put_abs(b, k as u32 * 480, 480, &[p(*lo)]).unwrap();
        }
        // 79-48 = 31 (a fifth + two octaves), 81-50 = 31: parallel fifths
        assert!(f.review().iter().any(|i| i.kind == "parallel" && i.message.contains("quintas")), "{:?}", f.review());
        // contrary motion is fine
        let mut g = Score::new("g", TimeSig { num: 4, den: 4 }, 0, 2);
        let b = g.add_part("Bajo", Clef::Bass, 0, false);
        for (k, (hi, lo)) in [(79, 48), (77, 53)].iter().enumerate() {
            g.put_abs(0, k as u32 * 480, 480, &[p(*hi)]).unwrap();
            g.put_abs(b, k as u32 * 480, 480, &[p(*lo)]).unwrap();
        }
        assert!(g.review().iter().all(|i| i.kind != "parallel"), "{:?}", g.review());
    }
}
