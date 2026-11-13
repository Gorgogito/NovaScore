//! Asistencia a la composición con reglas simbólicas (sin modelos ni descargas).
//!
//! - [`Score::add_accompaniment`] escribe una parte nueva (bajo, acordes o arpegio) a partir del cifrado de acordes.
//! - [`Score::review`] revisa la partitura y lista lo que suele ser un error: notas fuera de la tesitura del
//!   instrumento, voces cruzadas, quintas u octavas paralelas o directas entre dos líneas y saltos melódicos muy grandes.
//! - [`Score::harmonize`] propone un cifrado de acordes para la melodía de una parte (grados de la escala, con
//!   preferencia por los enlaces típicos y por cerrar en la tónica) y [`Score::set_progression`] escribe una progresión
//!   con números romanos (`I-V-vi-IV`, `ii-V-I`, `i-bVII-bVI-V`…).
//! - [`Score::vary_phrase`] sigue la melodía con una variación: modulación (transporte cromático) o inversión.
//! - [`Score::continue_phrase`] sigue la melodía de una parte repitiendo sus últimos compases como secuencia
//!   (subiendo o bajando por grados de la escala) y [`Score::end_with_cadence`] la cierra con la tónica.

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
    /// `range`, `crossing`, `parallel`, `direct` o `leap`.
    pub kind: &'static str,
    pub message: String,
}

/// Tesitura (MIDI de la nota más grave y de la más aguda) de un instrumento General MIDI, si la conocemos.
fn range_of(program: u8) -> Option<(i32, i32, &'static str)> {
    Some(match program {
        0..=7 => (21, 108, "piano"),
        8 => (60, 108, "celesta"),
        9 => (79, 108, "glockenspiel"),
        13 => (65, 108, "xilófono"),
        16..=20 => (36, 96, "órgano"),
        24..=31 => (40, 88, "guitarra"),
        46 => (24, 103, "arpa"),
        48 | 49 => (28, 103, "cuerdas"),
        52 => (48, 81, "coro"),
        32..=39 => (28, 67, "bajo"),
        40 => (55, 103, "violín"),
        41 => (48, 91, "viola"),
        42 => (36, 76, "violonchelo"),
        43 => (28, 67, "contrabajo"),
        56 => (54, 82, "trompeta"),
        57 => (40, 72, "trombón"),
        58 => (28, 58, "tuba"),
        60 => (41, 77, "trompa"),
        64 => (56, 87, "saxofón soprano"),
        65 => (49, 81, "saxofón alto"),
        66 => (44, 76, "saxofón tenor"),
        67 => (36, 69, "saxofón barítono"),
        69 => (52, 81, "corno inglés"),
        74 => (60, 96, "flauta dulce"),
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
        // 4. fifths and octaves reached in similar motion by a leap of the upper line (direct or "hidden")
        for (i, a) in lines.iter().enumerate() {
            for b in lines.iter().skip(i + 1) {
                let mut found = 0;
                let mut prev: Option<(i32, i32)> = None;
                for na in &a.notes {
                    let Some(nb) = b.notes.iter().find(|n| n.abs == na.abs) else { continue };
                    if let Some((pa, pb)) = prev {
                        let (ia, ib) = ((pa - pb).rem_euclid(12), (na.top - nb.top).rem_euclid(12));
                        let same_way = (na.top - pa).signum() * (nb.top - pb).signum() > 0;
                        let (upper_leap, upper_is_a) = if na.top >= nb.top { ((na.top - pa).abs() > 2, true) } else { ((nb.top - pb).abs() > 2, false) };
                        let _ = upper_is_a;
                        if same_way && ia != ib && (ib == 7 || ib == 0) && upper_leap && (na.top - nb.top).abs() >= 7 {
                            found += 1;
                            if found <= 2 {
                                let what = if ib == 7 { "quintas" } else { "octavas" };
                                issues.push(Issue { part: a.part, voice: a.voice, measure: na.measure, tick: na.tick, kind: "direct", message: format!("{what} directas (por salto en la voz superior) entre «{}» (voz {}) y «{}» (voz {}).", self.parts[a.part].name, a.voice + 1, self.parts[b.part].name, b.voice + 1) });
                            }
                        }
                    }
                    prev = Some((na.top, nb.top));
                }
            }
        }
        // 5. melodic leaps wider than an octave inside one line
        for l in &lines {
            let mut found = 0;
            for w in l.notes.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                if b.abs != a.end {
                    continue; // a rest in between: the leap is not sung or played in one breath
                }
                let leap = (b.top - a.top).abs();
                if leap > 12 {
                    found += 1;
                    if found <= 3 {
                        issues.push(Issue { part: l.part, voice: l.voice, measure: b.measure, tick: b.tick, kind: "leap", message: format!("Salto melódico de {leap} semitonos ({} → {}) en «{}»: más de una octava, difícil de cantar o tocar.", name_of(a.top), name_of(b.top), self.parts[l.part].name) });
                    }
                }
            }
        }
        issues.sort_by_key(|i| (i.measure, i.tick, i.part));
        issues
    }
}

const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// Key signature (sharps positive, flats negative, at most six of either) whose major tonic is the pitch class `pc`.
fn fifths_of_pc(pc: i32) -> i8 {
    let f = (pc.rem_euclid(12) * 7).rem_euclid(12) as i8;
    if f > 6 { f - 12 } else { f }
}

/// Moves a pitch `steps` degrees along the major scale of `tonic` (a pitch class). A note outside the scale keeps
/// its distance to the scale note below it.
fn shift_degrees(midi: i32, tonic: i32, steps: i32) -> i32 {
    let (d, extra) = degree_index(midi, tonic);
    from_degree_index(d + steps, tonic) + extra
}

/// Position of a pitch on the major scale of `tonic` as a running degree number (7 per octave) plus 1 if it is a
/// chromatic note sitting a semitone above the degree.
fn degree_index(midi: i32, tonic: i32) -> (i32, i32) {
    let rel = midi - tonic;
    let (base, extra) = if MAJOR.contains(&rel.rem_euclid(12)) { (rel, 0) } else { (rel - 1, 1) };
    let (oct, r) = (base.div_euclid(12), base.rem_euclid(12));
    (oct * 7 + MAJOR.iter().position(|&x| x == r).unwrap_or(0) as i32, extra)
}

fn from_degree_index(total: i32, tonic: i32) -> i32 {
    tonic + total.div_euclid(7) * 12 + MAJOR[total.rem_euclid(7) as usize]
}

/// How [`Score::vary_phrase`] changes the pitches of the repeated measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseMove {
    /// Sequence: `n` degrees of the scale higher (negative: lower).
    Degrees(i32),
    /// Modulation: `n` semitones higher (negative: lower), keeping every interval of the phrase.
    Semitones(i32),
    /// Mirror inversion along the scale around the first note of the phrase.
    Invert,
}

impl PhraseMove {
    fn apply(self, midi: i32, tonic: i32, pivot: i32) -> i32 {
        match self {
            PhraseMove::Degrees(s) => shift_degrees(midi, tonic, s),
            PhraseMove::Semitones(n) => midi + n,
            PhraseMove::Invert => {
                let ((d, extra), (p, _)) = (degree_index(midi, tonic), degree_index(pivot, tonic));
                from_degree_index(2 * p - d, tonic) + extra
            }
        }
    }
}

impl Score {
    /// Pitch class of the major tonic of the key in force at measure `m`.
    fn tonic_at(&self, m: usize) -> i32 {
        (self.key_at(m) as i32 * 7).rem_euclid(12)
    }

    /// Last measure of the part with any note in it.
    fn last_written_measure(&self, part: usize) -> Option<usize> {
        let p = self.parts.get(part)?;
        (0..p.measures.len()).rev().find(|&m| (0..MAX_VOICES).any(|v| p.measures[m].voice(v).into_iter().flatten().any(|e| matches!(e.content, Content::Note { .. }))))
    }

    /// Continues the music of `part` by repeating its last `phrase` measures `steps` scale degrees higher (negative:
    /// lower), as a melodic sequence. The measures are appended to every part (the others stay silent there).
    /// Returns the index of the first new measure.
    pub fn continue_phrase(&mut self, part: usize, phrase: usize, steps: i32) -> Result<usize, String> {
        self.vary_phrase(part, phrase, PhraseMove::Degrees(steps))
    }

    /// Like [`Score::continue_phrase`] but with any [`PhraseMove`]: a modulation moves the phrase by semitones (the key
    /// signature changes where the new bars start, to the key of the new tonic) and an inversion mirrors it along the scale.
    pub fn vary_phrase(&mut self, part: usize, phrase: usize, how: PhraseMove) -> Result<usize, String> {
        if phrase == 0 || matches!(how, PhraseMove::Degrees(0) | PhraseMove::Semitones(0)) {
            return Err("La frase y el desplazamiento no pueden ser cero.".into());
        }
        let last = self.last_written_measure(part).ok_or("Esa parte no tiene notas que continuar.")?;
        if last + 1 < phrase {
            return Err(format!("Solo hay {} compás(es) escritos: elige una frase más corta.", last + 1));
        }
        let first = last + 1 - phrase;
        let tonic = self.tonic_at(last);
        let (sharps, new_key) = match how {
            // the phrase lands in a new key: spell it the way that key's own signature would
            PhraseMove::Semitones(n) => {
                let f = fifths_of_pc(tonic + n);
                (f >= 0, Some(f))
            }
            _ => (self.key_at(last) >= 0, None),
        };
        let pivot = (first..=last)
            .find_map(|m| {
                self.parts[part].measures[m].voice(0).into_iter().flatten().find_map(|e| match &e.content {
                    Content::Note { pitches, .. } => pitches.iter().map(|q| q.midi()).max(),
                    _ => None,
                })
            })
            .unwrap_or(60);
        let total = self.parts[part].measures.len();
        // the copies keep the original lengths: a different time signature in between is not handled
        let lens: Vec<u32> = (first..=last).map(|m| self.measure_len(m)).collect();
        let new_start = total;
        if (0..phrase).any(|i| self.measure_len(new_start + i) != lens[i]) {
            return Err("El compás cambia justo ahí: la secuencia necesita el mismo compás que la frase.".into());
        }
        let range = range_of(self.parts[part].program);
        let mut copies: Vec<Measure> = Vec::new();
        for m in first..=last {
            let mut c = self.parts[part].measures[m].clone();
            c.extras.clear();
            c.start_repeat = false;
            c.end_repeat = 0;
            c.markers.clear();
            c.jump = None;
            for e in c.events.iter_mut().chain(c.voices.iter_mut().flatten()) {
                e.extras.clear();
                e.lyrics.clear();
                e.text = None;
                e.strings.clear();
                if let Content::Note { pitches, tie } = &e.content {
                    let moved: Vec<Pitch> = pitches.iter().map(|q| Pitch::from_midi(how.apply(q.midi(), tonic, pivot), sharps)).collect();
                    if let Some((lo, hi, name)) = range {
                        if moved.iter().any(|q| q.midi() < lo || q.midi() > hi) {
                            return Err(format!("La secuencia saldría de la tesitura de «{}» ({name}): prueba en sentido contrario.", self.parts[part].name));
                        }
                    }
                    e.content = Content::note(moved, *tie);
                }
            }
            copies.push(c);
        }
        // slurs and hairpins inside the phrase come along
        let moved_spanners: Vec<Spanner> = self.parts[part]
            .spanners
            .iter()
            .filter(|s| matches!(s.kind, SpannerKind::Slur | SpannerKind::Crescendo | SpannerKind::Decrescendo) && s.start.measure >= first && s.end.measure <= last)
            .map(|s| {
                let shift = new_start - first;
                Spanner { start: SpPos { measure: s.start.measure + shift, tick: s.start.tick }, end: SpPos { measure: s.end.measure + shift, tick: s.end.tick }, ..*s }
            })
            .collect();
        self.add_measures(phrase);
        let p = &mut self.parts[part];
        for (i, c) in copies.into_iter().enumerate() {
            p.measures[new_start + i] = c;
        }
        p.spanners.extend(moved_spanners);
        if let Some(f) = new_key {
            self.set_key_at(new_start, f);
        }
        Ok(new_start)
    }

    /// Adds one measure to the end of `part` with a held tonic (the one nearest to its last note): a plain cadence.
    /// Returns the index of the new measure.
    pub fn end_with_cadence(&mut self, part: usize) -> Result<usize, String> {
        let last = self.last_written_measure(part).ok_or("Esa parte no tiene notas que cerrar.")?;
        let top = self.parts[part].measures[last].voice(0).into_iter().flatten().rev().find_map(|e| match &e.content {
            Content::Note { pitches, .. } => pitches.iter().map(|q| q.midi()).max(),
            _ => None,
        });
        let top = top.unwrap_or(60);
        let tonic = self.tonic_at(last);
        // the tonic closest to the last note (ties go down)
        let mut best = top - (top - tonic).rem_euclid(12);
        if top - best > 6 {
            best += 12;
        }
        let at = self.parts[part].measures.len();
        self.add_measures(1);
        let len = self.measure_len(at);
        let abs = self.measure_starts(at + 1)[at];
        let sharps = self.key_at(last) >= 0;
        self.put_abs(part, abs, len, &[Pitch::from_midi(best, sharps)]).map_err(|e| format!("{e:?}"))?;
        Ok(at)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Harmonization

/// Degrees available when harmonizing: (semitones above the tonic, quality, degree number).
const MAJOR_DEGREES: [(u8, Quality, u8); 6] = [(0, Quality::Major, 1), (2, Quality::Minor, 2), (4, Quality::Minor, 3), (5, Quality::Major, 4), (7, Quality::Major, 5), (9, Quality::Minor, 6)];
const MINOR_DEGREES: [(u8, Quality, u8); 7] = [(0, Quality::Minor, 1), (2, Quality::Dim, 2), (3, Quality::Major, 3), (5, Quality::Minor, 4), (7, Quality::Major, 5), (8, Quality::Major, 6), (10, Quality::Major, 7)];

/// Taste for going from degree `a` to degree `b`: the usual functional links score high, retrogressions low.
fn link(a: u8, b: u8) -> f32 {
    match (a, b) {
        _ if a == b => -1.5,
        (5, 1) => 3.0,
        (2, 5) => 2.5,
        (4, 5) => 2.0,
        (5, 6) | (6, 4) | (6, 2) | (3, 6) | (7, 3) => 1.5,
        (1, 4) | (1, 6) | (4, 1) | (6, 7) | (7, 1) => 1.0,
        (1, 5) | (4, 2) => 0.5,
        (5, 4) | (5, 2) => -1.5,
        _ => 0.0,
    }
}

/// Roman numeral (`I`, `ii`, `bVII`, `V7`, `vii°`…) → (semitones above the tonic, quality). Upper case is major, lower
/// case minor; a leading `b` or `#` lowers or raises the degree; `7` makes it a seventh chord.
fn parse_roman(s: &str) -> Option<(i32, Quality)> {
    let mut t = s;
    let mut acc = 0;
    if let Some(r) = t.strip_prefix('b').or_else(|| t.strip_prefix('♭')) {
        acc = -1;
        t = r;
    } else if let Some(r) = t.strip_prefix('#').or_else(|| t.strip_prefix('♯')) {
        acc = 1;
        t = r;
    }
    let letters: String = t.chars().take_while(|c| matches!(c, 'I' | 'V' | 'i' | 'v')).collect();
    let deg = match letters.to_uppercase().as_str() {
        "I" => 0,
        "II" => 1,
        "III" => 2,
        "IV" => 3,
        "V" => 4,
        "VI" => 5,
        "VII" => 6,
        _ => return None,
    };
    let upper = letters.chars().next()?.is_uppercase();
    let q = match (&t[letters.len()..], upper) {
        ("", true) => Quality::Major,
        ("", false) => Quality::Minor,
        ("7", true) => Quality::Dominant7,
        ("7", false) => Quality::Minor7,
        ("maj7", true) | ("Δ", true) => Quality::Major7,
        ("°", _) | ("dim", _) => Quality::Dim,
        ("°7", _) | ("dim7", _) => Quality::Dim7,
        _ => return None,
    };
    Some(((MAJOR[deg] + acc).rem_euclid(12), q))
}

impl Score {
    /// Tonic of the harmonization at measure `m`: the major tonic of the key signature there, or its relative minor.
    fn mode_tonic(&self, minor: bool, m: usize) -> i32 {
        if minor { (self.tonic_at(m) + 9) % 12 } else { self.tonic_at(m) }
    }

    /// Stores one chord per measure (consecutive repeats are merged) as the chord symbols of the score, fixed by hand
    /// so «Actualizar acordes» keeps them.
    fn set_bar_chords(&mut self, chords: &[(i32, Quality)]) {
        let mut marks: Vec<ChordMark> = Vec::new();
        for (m, &(root, quality)) in chords.iter().enumerate() {
            let root = root.rem_euclid(12) as u8;
            if marks.last().map_or(false, |l| (l.root, l.quality) == (root, quality)) {
                continue;
            }
            marks.push(ChordMark { measure: m, tick: 0, root, quality, bass: root, manual: true });
        }
        self.chords = marks;
    }

    /// Writes the chord symbols for a progression given as roman numerals separated by hyphens or spaces, one chord per
    /// measure and repeated to the end of the score. `minor` takes the relative minor of the key signature as tonic
    /// (write its degrees as in a major key: `i-bVII-bVI-V`). Returns the number of chord symbols written.
    pub fn set_progression(&mut self, text: &str, minor: bool) -> Result<usize, String> {
        let cycle: Option<Vec<(i32, Quality)>> = text.split(|c: char| c == '-' || c == '–' || c.is_whitespace()).filter(|w| !w.is_empty()).map(parse_roman).collect();
        let cycle = cycle.ok_or("No entiendo la progresión: usa números romanos separados por guiones, como I-V-vi-IV o ii-V7-I.")?;
        if cycle.is_empty() {
            return Err("La progresión está vacía.".into());
        }
        let n = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        let bars: Vec<(i32, Quality)> = (0..n).map(|m| (self.mode_tonic(minor, m) + cycle[m % cycle.len()].0, cycle[m % cycle.len()].1)).collect();
        self.set_bar_chords(&bars);
        Ok(self.chords.len())
    }

    /// Proposes chord symbols for the melody of `part`, one chord per measure, chosen among the degrees of the key
    /// (major, or its relative minor when `minor`). Each bar scores the share of its notes (weighted by length and by
    /// beat) that belong to the chord, plus a taste for the usual links (V–I, ii–V, IV–V…), for starting on the tonic
    /// and for closing V–I; the best path is found with dynamic programming. Returns the number of chord symbols.
    pub fn harmonize(&mut self, part: usize, minor: bool) -> Result<usize, String> {
        let p = self.parts.get(part).ok_or("La parte no existe.")?;
        let n = p.measures.len();
        let mut weights: Vec<[f32; 12]> = vec![[0.0; 12]; n];
        let mut any = false;
        for (m, w) in weights.iter_mut().enumerate() {
            for v in 0..MAX_VOICES {
                for e in p.measures[m].voice(v).into_iter().flatten() {
                    if let Content::Note { pitches, .. } = &e.content {
                        let beat = if e.start == 0 { 2.0 } else if e.start % PPQ == 0 { 1.5 } else { 1.0 };
                        for q in pitches {
                            w[q.midi().rem_euclid(12) as usize] += e.ticks as f32 * beat;
                        }
                        any = true;
                    }
                }
            }
        }
        if !any {
            return Err("Esa parte no tiene notas que armonizar.".into());
        }
        let tonics: Vec<i32> = (0..n).map(|m| self.mode_tonic(minor, m)).collect();
        let cands: &[(u8, Quality, u8)] = if minor { &MINOR_DEGREES } else { &MAJOR_DEGREES };
        let fit = |m: usize, c: usize| -> f32 {
            let total: f32 = weights[m].iter().sum();
            if total == 0.0 {
                return 0.0;
            }
            let (off, q, num) = cands[c];
            let root = (tonics[m] + off as i32).rem_euclid(12);
            let inside: f32 = tones(q).iter().map(|&t| weights[m][(root + t as i32).rem_euclid(12) as usize]).sum();
            let mut s = (inside - 0.7 * (total - inside)) / total * 4.0;
            if m == 0 && num == 1 {
                s += 1.5;
            }
            if m + 1 == n {
                s += match num { 1 => 3.0, 5 => 0.5, _ => 0.0 };
            }
            if m + 2 == n && num == 5 {
                s += 1.0;
            }
            s
        };
        let mut best: Vec<Vec<f32>> = vec![vec![0.0; cands.len()]; n];
        let mut from: Vec<Vec<usize>> = vec![vec![0; cands.len()]; n];
        for c in 0..cands.len() {
            best[0][c] = fit(0, c);
        }
        for m in 1..n {
            for c in 0..cands.len() {
                let (prev, score) = (0..cands.len()).map(|q| (q, best[m - 1][q] + link(cands[q].2, cands[c].2))).fold((0, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
                best[m][c] = score + fit(m, c);
                from[m][c] = prev;
            }
        }
        let mut c = (0..cands.len()).fold(0, |a, b| if best[n - 1][b] > best[n - 1][a] { b } else { a });
        let mut path = vec![0usize; n];
        for m in (0..n).rev() {
            path[m] = c;
            c = from[m][c];
        }
        let bars: Vec<(i32, Quality)> = path.iter().enumerate().map(|(m, &c)| (tonics[m] + cands[c].0 as i32, cands[c].1)).collect();
        self.set_bar_chords(&bars);
        Ok(self.chords.len())
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

    fn pitches_of(s: &Score, part: usize, m: usize) -> Vec<i32> {
        s.parts[part].measures[m].events.iter().filter_map(|e| if let Content::Note { pitches, .. } = &e.content { Some(pitches[0].midi()) } else { None }).collect()
    }

    fn two_bars() -> Score {
        let mut s = Score::new("q", TimeSig { num: 4, den: 4 }, 0, 2);
        for (k, n) in [60, 62, 64, 65, 67, 65, 64, 62].iter().enumerate() {
            s.put_abs(0, k as u32 * 480, 480, &[p(*n)]).unwrap();
        }
        s
    }

    #[test]
    fn a_phrase_continues_as_a_sequence_along_the_scale() {
        let mut s = two_bars();
        s.add_part("Otra", Clef::Treble, 0, false);
        // two bars lower by a degree: C D E F -> B C D E, G F E D -> F E D C
        let first = s.continue_phrase(0, 2, -1).unwrap();
        assert_eq!(first, 2);
        assert_eq!(s.parts[0].measures.len(), 4);
        assert_eq!(s.parts[1].measures.len(), 4, "every part grows");
        assert_eq!(pitches_of(&s, 0, 2), vec![59, 60, 62, 64]);
        assert_eq!(pitches_of(&s, 0, 3), vec![65, 64, 62, 60]);
        // and upwards: the new bars continue from the last written ones
        let mut u = two_bars();
        u.continue_phrase(0, 1, 2).unwrap();
        assert_eq!(pitches_of(&u, 0, 2), vec![67, 65, 64, 62].iter().map(|x| shift_degrees(*x, 0, 2)).collect::<Vec<_>>());
        // a chromatic note keeps its distance to the note below
        assert_eq!(shift_degrees(61, 0, 1), 63);
        // a part with a narrower range refuses to leave it, leaving the score untouched
        let mut v = two_bars();
        v.parts[0].program = 40; // violin starts at G3 = 55; the phrase would go below with a long descent
        for k in 0..8 {
            v.put_abs(0, k * 480, 480, &[p(55 + k as i32)]).unwrap();
        }
        let before = v.parts[0].measures.len();
        assert!(v.continue_phrase(0, 2, -2).is_err());
        assert_eq!(v.parts[0].measures.len(), before);
        assert!(two_bars().continue_phrase(0, 5, -1).is_err(), "a longer phrase than the music is refused");
    }

    #[test]
    fn the_cadence_closes_on_the_nearest_tonic() {
        let mut s = two_bars();
        let m = s.end_with_cadence(0).unwrap();
        assert_eq!(m, 2);
        let n = &s.parts[0].measures[2].events;
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].ticks, 1920);
        // the melody ended on D4 (62): the nearest C is C4
        assert_eq!(pitches_of(&s, 0, 2), vec![60]);
        // in G major (one sharp) the tonic is G
        let mut g = Score::new("g", TimeSig { num: 4, den: 4 }, 1, 1);
        g.put_abs(0, 0, 1920, &[p(71)]).unwrap();
        g.end_with_cadence(0).unwrap();
        assert_eq!(pitches_of(&g, 0, 1), vec![67]);
    }

    #[test]
    fn review_flags_direct_fifths_and_wide_leaps() {
        // upper voice leaps from E to the octave above the bass that moved in the same direction: direct octave
        let mut s = Score::new("d", TimeSig { num: 4, den: 4 }, 0, 1);
        let b = s.add_part("Bajo", Clef::Bass, 0, false);
        for (k, (hi, lo)) in [(64, 48), (72, 60)].iter().enumerate() {
            s.put_abs(0, k as u32 * 480, 480, &[p(*hi)]).unwrap();
            s.put_abs(b, k as u32 * 480, 480, &[p(*lo)]).unwrap();
        }
        let issues = s.review();
        assert!(issues.iter().any(|i| i.kind == "direct" && i.message.contains("octavas")), "{issues:?}");
        assert!(issues.iter().all(|i| i.kind != "parallel"));
        // a leap of more than an octave between notes that follow each other, but not across a rest
        let mut l = Score::new("l", TimeSig { num: 4, den: 4 }, 0, 1);
        l.put_abs(0, 0, 480, &[p(60)]).unwrap();
        l.put_abs(0, 480, 480, &[p(76)]).unwrap();
        l.put_abs(0, 1440, 480, &[p(60)]).unwrap();
        let issues = l.review();
        assert_eq!(issues.iter().filter(|i| i.kind == "leap").count(), 1, "{issues:?}");
    }

    #[test]
    fn a_phrase_can_be_inverted_or_modulated() {
        // last bar G F E D mirrored around G along C major: G A B C
        let mut s = two_bars();
        s.vary_phrase(0, 1, PhraseMove::Invert).unwrap();
        assert_eq!(pitches_of(&s, 0, 2), vec![67, 69, 71, 72]);
        // a modulation to the dominant keeps every interval: +7 semitones
        let mut m = two_bars();
        m.vary_phrase(0, 1, PhraseMove::Semitones(7)).unwrap();
        assert_eq!(pitches_of(&m, 0, 2), vec![74, 72, 71, 69]);
        assert_eq!((m.key_at(1), m.key_at(2)), (0, 1), "the new bars are in G major: one sharp");
        // down to the relative minor: -3 semitones, with accidentals where the old scale does not reach
        let mut r = two_bars();
        r.vary_phrase(0, 2, PhraseMove::Semitones(-3)).unwrap();
        assert_eq!(pitches_of(&r, 0, 2), vec![57, 59, 61, 62]);
        assert_eq!(r.key_at(2), 3, "a minor third lower lands in A major");
        // a sequence or an inversion stays in the key
        let mut q = two_bars();
        q.vary_phrase(0, 1, PhraseMove::Invert).unwrap();
        assert!(q.key_changes.is_empty());
        // a phrase that modulates again continues from the new key
        m.vary_phrase(0, 1, PhraseMove::Semitones(7)).unwrap();
        assert_eq!((m.key_at(2), m.key_at(3)), (1, 2));
        assert!(two_bars().vary_phrase(0, 1, PhraseMove::Semitones(0)).is_err());
    }

    fn roots(s: &Score) -> Vec<(usize, u8, Quality)> {
        s.chords.iter().map(|c| (c.measure, c.root, c.quality)).collect()
    }

    #[test]
    fn a_progression_in_roman_numerals_is_written_bar_by_bar() {
        // G major (one sharp): I-V-vi-IV = G D Em C, repeated over five bars
        let mut s = Score::new("p", TimeSig { num: 4, den: 4 }, 1, 5);
        assert_eq!(s.set_progression("I-V-vi-IV", false).unwrap(), 5);
        assert_eq!(roots(&s), vec![(0, 7, Quality::Major), (1, 2, Quality::Major), (2, 4, Quality::Minor), (3, 0, Quality::Major), (4, 7, Quality::Major)]);
        assert!(s.chords.iter().all(|c| c.manual && c.bass == c.root), "fixed by hand, so «Actualizar acordes» keeps them");
        // A minor (no signature): i-bVII-bVI-V = Am G F E
        let mut m = Score::new("m", TimeSig { num: 4, den: 4 }, 0, 4);
        m.set_progression("i-bVII-bVI-V", true).unwrap();
        assert_eq!(roots(&m), vec![(0, 9, Quality::Minor), (1, 7, Quality::Major), (2, 5, Quality::Major), (3, 4, Quality::Major)]);
        // sevenths and merged repeats: I7 I7 IV7 -> two symbols
        let mut b = Score::new("b", TimeSig { num: 4, den: 4 }, 0, 3);
        assert_eq!(b.set_progression("I7 I7 IV7", false).unwrap(), 2);
        assert_eq!(b.chords[0].quality, Quality::Dominant7);
        assert!(b.set_progression("I-X-V", false).is_err());
        assert!(b.set_progression("  ", false).is_err());
        // and the accompaniment reads them
        let pi = b.add_accompaniment(Accompaniment::Bass).unwrap();
        assert!(pi > 0 || b.parts.len() > 1);
    }

    #[test]
    fn a_melody_is_harmonized_with_the_degrees_it_outlines() {
        let mut s = Score::new("h", TimeSig { num: 4, den: 4 }, 0, 4);
        let bars: [[i32; 4]; 3] = [[60, 64, 67, 64], [65, 69, 60, 69], [67, 71, 62, 71]];
        for (m, b) in bars.iter().enumerate() {
            for (k, n) in b.iter().enumerate() {
                s.put_abs(0, m as u32 * 1920 + k as u32 * 480, 480, &[p(*n)]).unwrap();
            }
        }
        s.put_abs(0, 3 * 1920, 1920, &[p(60)]).unwrap();
        assert_eq!(s.harmonize(0, false).unwrap(), 4);
        // C F G C
        assert_eq!(roots(&s), vec![(0, 0, Quality::Major), (1, 5, Quality::Major), (2, 7, Quality::Major), (3, 0, Quality::Major)]);
        // an empty part has nothing to harmonize
        let mut e = Score::new("e", TimeSig { num: 4, den: 4 }, 0, 2);
        assert!(e.harmonize(0, false).is_err());
        // a melody that closes on A in A minor ends on the tonic chord
        let mut a = Score::new("a", TimeSig { num: 4, den: 4 }, 0, 2);
        a.put_abs(0, 0, 1920, &[p(64)]).unwrap();
        a.put_abs(0, 1920, 1920, &[p(69)]).unwrap();
        a.harmonize(0, true).unwrap();
        assert_eq!(a.chords.last().map(|c| (c.root, c.quality)), Some((9, Quality::Minor)), "{:?}", roots(&a));
    }

    #[test]
    fn harmony_follows_the_key_of_each_measure() {
        // two bars in C, then two in G (one sharp): G B D must be the tonic there, not the dominant
        let mut s = Score::new("k", TimeSig { num: 4, den: 4 }, 0, 4);
        s.set_key_at(2, 1);
        for (m, ns) in [[60, 64, 67, 64], [60, 64, 67, 72]].iter().enumerate() {
            for (k, n) in ns.iter().enumerate() {
                s.put_abs(0, m as u32 * 1920 + k as u32 * 480, 480, &[p(*n)]).unwrap();
            }
        }
        s.put_abs(0, 2 * 1920, 1920, &[p(71)]).unwrap();
        s.put_abs(0, 3 * 1920, 1920, &[p(67)]).unwrap();
        s.harmonize(0, false).unwrap();
        assert_eq!(s.chords.last().map(|c| (c.measure, c.root)), Some((3, 7)), "{:?}", roots(&s));
        // a progression is transposed to the key in force: I-IV over C | C | G | G
        let mut t = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 4);
        t.set_key_at(2, 1);
        t.set_progression("I-IV", false).unwrap();
        assert_eq!(roots(&t).iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 5, 7, 0]);
    }
}
