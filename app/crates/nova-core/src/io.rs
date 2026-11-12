//! Playback flattening, MIDI export and MusicXML import/export.

use crate::chords::{ChordMark, Quality};
use crate::*;
use serde::Serialize;
use std::fmt::Write as _;

/// A sounding note with ties already merged. Ticks are absolute from the start of the score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sound {
    pub part: usize,
    /// MIDI channel (0-15) of the part.
    pub channel: u8,
    pub start: u32,
    pub ticks: u32,
    pub midi: i32,
    /// MIDI velocity: dynamics set it, hairpins ramp it and accents raise it.
    pub velocity: u8,
}

/// Velocity of a part over time: the last dynamic marking holds until the next one, and a
/// crescendo / decrescendo ramps towards the next marking (or 25 steps away without one).
struct Velocity {
    points: Vec<(u32, i32)>,
    ramps: Vec<(u32, u32, bool)>, // start, end, crescendo
}

impl Velocity {
    const DEFAULT: i32 = 80;

    fn new(part: &Part, starts: &[u32]) -> Velocity {
        let mut points: Vec<(u32, i32)> = Vec::new();
        for (mi, m) in part.measures.iter().enumerate() {
            for v in 0..MAX_VOICES {
                for e in m.voice(v).into_iter().flatten() {
                    if let Some(d) = e.dynamic {
                        points.push((starts[mi] + e.start, d.velocity() as i32));
                    }
                }
            }
        }
        points.sort_by_key(|p| p.0);
        let at = |p: SpPos| starts.get(p.measure).copied().unwrap_or(0) + p.tick;
        let ramps = part.spanners.iter().filter(|s| matches!(s.kind, SpannerKind::Crescendo | SpannerKind::Decrescendo)).map(|s| (at(s.start), at(s.end), s.kind == SpannerKind::Crescendo)).collect();
        Velocity { points, ramps }
    }

    fn held(&self, abs: u32) -> i32 {
        self.points.iter().rev().find(|p| p.0 <= abs).map_or(Self::DEFAULT, |p| p.1)
    }

    fn at(&self, abs: u32) -> i32 {
        let base = self.held(abs);
        for &(a, b, cresc) in &self.ramps {
            if abs >= a && abs < b {
                let from = self.held(a);
                let target = match self.points.iter().find(|p| p.0 >= b) {
                    Some(p) if (p.1 > from) == cresc && p.1 != from => p.1,
                    _ => from + if cresc { 25 } else { -25 },
                };
                let t = (abs - a) as f64 / (b - a) as f64;
                return (from as f64 + (target - from) as f64 * t).round() as i32;
            }
        }
        base
    }
}

/// Start tick (in playing time) of every measure of the unfolded playback order.
fn play_offsets(score: &Score, order: &[usize]) -> Vec<u32> {
    let mut off = Vec::with_capacity(order.len());
    let mut pos = 0;
    for &mi in order {
        off.push(pos);
        pos += score.measure_len(mi);
    }
    off
}

/// Pitch a trill alternates with: the next note of the major scale of the key above `midi`.
fn trill_upper(midi: i32, fifths: i8) -> i32 {
    let tonic = (fifths as i32 * 7).rem_euclid(12);
    let in_scale = |m: i32| [0, 2, 4, 5, 7, 9, 11].contains(&(m - tonic).rem_euclid(12));
    (1..=2).map(|d| midi + d).find(|&m| in_scale(m)).unwrap_or(midi + 2)
}

/// Pitch a step below `midi` in the major scale of the key.
fn scale_lower(midi: i32, fifths: i8) -> i32 {
    let tonic = (fifths as i32 * 7).rem_euclid(12);
    let in_scale = |m: i32| [0, 2, 4, 5, 7, 9, 11].contains(&(m - tonic).rem_euclid(12));
    (1..=2).map(|d| midi - d).find(|&m| in_scale(m)).unwrap_or(midi - 2)
}

/// An ornament written out as the notes it plays inside the note's length: (pitch, ticks) pairs.
/// `None` when the note is too short to hold it.
fn ornament_notes(a: Artic, s: &Sound, fifths: i8) -> Option<Vec<(i32, u32)>> {
    let (up, down) = (trill_upper(s.midi, fifths), scale_lower(s.midi, fifths));
    let t = s.ticks;
    match a {
        Artic::Mordent | Artic::ShortTrill => {
            let len = 60;
            (t >= 3 * len).then(|| vec![(s.midi, len), (if a == Artic::Mordent { down } else { up }, len), (s.midi, t - 2 * len)])
        }
        Artic::Turn | Artic::InvertedTurn => {
            let len = (t / 4).min(90);
            (len >= 40).then(|| {
                let (first, third) = if a == Artic::Turn { (up, down) } else { (down, up) };
                vec![(first, len), (s.midi, len), (third, len), (s.midi, t - 3 * len)]
            })
        }
        _ => None,
    }
}

/// The notes of the score as they play: repeats and voltas are unfolded, tied notes are merged,
/// dynamics, articulations and trills shape velocity and length. `start` is playing time.
pub fn sounds(score: &Score) -> Vec<Sound> {
    let starts = score.measure_starts(score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0));
    let order = score.playback_order();
    let offsets = play_offsets(score, &order);
    let mut out: Vec<Sound> = Vec::new();
    // per sound: position in the written score (for trills) and the voice
    let mut written_at: Vec<(u32, usize)> = Vec::new();
    let chans = score.midi_channels();
    let mut gates: Vec<(usize, u32, f64)> = Vec::new(); // sound index, written ticks, gate
    let mut ornaments: std::collections::HashMap<usize, Artic> = Default::default(); // sound index -> ornament
    for (pi, part) in score.parts.iter().enumerate() {
      let velocity = Velocity::new(part, &starts);
      for voice in 0..MAX_VOICES {
        let mut open: Vec<(i32, usize)> = Vec::new();
        let mut prev_mi: Option<usize> = None;
        for (k, &mi) in order.iter().enumerate() {
            // a tie only continues into the measure that follows it in the score
            if prev_mi.map_or(true, |p| p + 1 != mi) {
                open.clear();
            }
            prev_mi = Some(mi);
            let Some(evs) = part.measures.get(mi).and_then(|m| m.voice(voice)) else {
                open.clear();
                continue;
            };
            for e in evs {
                let abs = starts[mi] + e.start;
                let play = offsets[k] + e.start;
                match &e.content {
                    Content::Rest => open.clear(),
                    Content::Note { pitches, tie } => {
                        let mut next = Vec::new();
                        for p in pitches {
                            let midi = p.midi();
                            let idx = match open.iter().find(|(x, _)| *x == midi) {
                                Some(&(_, idx)) => {
                                    out[idx].ticks += e.ticks;
                                    idx
                                }
                                None => {
                                    let boost = e.articulations.iter().map(|a| a.velocity_boost()).max().unwrap_or(0);
                                    let vel = (velocity.at(abs) + boost).clamp(1, 127) as u8;
                                    out.push(Sound { part: pi, channel: chans[pi], start: play, ticks: e.ticks, midi, velocity: vel });
                                    written_at.push((abs, voice));
                                    let gate = e.articulations.iter().map(|a| a.gate()).fold(1.0, f64::min);
                                    if gate < 1.0 {
                                        gates.push((out.len() - 1, e.ticks, gate));
                                    }
                                    if let Some(o) = e.articulations.iter().find(|a| a.is_ornament()) {
                                        ornaments.insert(out.len() - 1, *o);
                                    }
                                    out.len() - 1
                                }
                            };
                            if *tie {
                                next.push((midi, idx));
                            }
                        }
                        open = next;
                    }
                }
            }
        }
      }
    }
    // Staccato and friends shorten notes, except those joined by a tie.
    for (i, written, gate) in gates {
        if out[i].ticks == written {
            out[i].ticks = ((written as f64 * gate).round() as u32).max(MIN_TICKS / 2);
        }
    }
    // Trills: the note alternates with its upper neighbour in 32nd notes.
    let any_trill = score.parts.iter().any(|p| p.spanners.iter().any(|s| s.kind == SpannerKind::Trill));
    if !any_trill && ornaments.is_empty() {
        return out;
    }
    let mut result = Vec::with_capacity(out.len());
    for (i, (s, (abs, voice))) in out.into_iter().zip(written_at).enumerate() {
        let part = &score.parts[s.part];
        // Mordents and turns are written out as the notes they play.
        if let (Some(&o), false) = (ornaments.get(&i), part.percussion) {
            if let Some(notes) = ornament_notes(o, &s, score.fifths) {
                let mut t = 0;
                for (midi, len) in notes {
                    result.push(Sound { start: s.start + t, ticks: len, midi, ..s.clone() });
                    t += len;
                }
                continue;
            }
        }
        let trilled = !part.percussion
            && part.spanners.iter().any(|sp| {
                let at = |p: SpPos| starts.get(p.measure).copied().unwrap_or(0) + p.tick;
                sp.kind == SpannerKind::Trill && sp.voice as usize == voice && abs >= at(sp.start) && abs < at(sp.end)
            });
        if !trilled || s.ticks < 2 * 60 {
            result.push(s);
            continue;
        }
        let upper = trill_upper(s.midi, score.fifths);
        let (mut t, mut up) = (0u32, false);
        while t < s.ticks {
            let len = 60.min(s.ticks - t);
            result.push(Sound { start: s.start + t, ticks: len, midi: if up { upper } else { s.midi }, ..s.clone() });
            t += len;
            up = !up;
        }
    }
    result
}

/// Sustain pedal change (MIDI controller 64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PedalEvent {
    pub part: usize,
    pub channel: u8,
    pub at: u32,
    pub down: bool,
}

/// `<direction>` of a segno, coda, "To Coda" or fine marker.
fn xml_nav_marker(mk: Marker) -> String {
    let (kind, sound) = match mk {
        Marker::Segno => ("<segno/>", "segno=\"segno\""),
        Marker::Coda => ("<coda/>", "coda=\"coda\""),
        Marker::ToCoda => ("<words>To Coda</words>", "tocoda=\"coda\""),
        Marker::Fine => ("<words>Fine</words>", "fine=\"yes\""),
    };
    format!("      <direction placement=\"above\"><direction-type>{kind}</direction-type><sound {sound}/></direction>
")
}

/// Pedal presses and releases, in time order.
pub fn pedals(score: &Score) -> Vec<PedalEvent> {
    let starts = score.measure_starts(score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0));
    let order = score.playback_order();
    let offsets = play_offsets(score, &order);
    let chans = score.midi_channels();
    let mut out = Vec::new();
    for (pi, part) in score.parts.iter().enumerate() {
        let at = |p: SpPos| starts.get(p.measure).copied().unwrap_or(0) + p.tick;
        let marks: Vec<(u32, bool)> = part.spanners.iter().filter(|s| s.kind == SpannerKind::Pedal).flat_map(|sp| [(at(sp.start), true), (at(sp.end), false)]).collect();
        // every time a measure is played, the presses and releases that fall in it are heard
        for (k, &mi) in order.iter().enumerate() {
            let (ms, me) = (starts[mi], starts[mi] + score.measure_len(mi));
            for &(abs, down) in &marks {
                let inside = if down { abs >= ms && abs < me } else { abs > ms && abs <= me };
                if inside {
                    out.push(PedalEvent { part: pi, channel: chans[pi], at: offsets[k] + (abs - ms), down });
                }
            }
        }
    }
    // a release at the same tick as a press goes first
    out.sort_by_key(|e| (e.at, e.down));
    out
}

/// Pedal events that should be heard: honours mute and solo like `audible_sounds`.
pub fn audible_pedals(score: &Score) -> Vec<PedalEvent> {
    let any_solo = score.parts.iter().any(|p| p.solo);
    pedals(score).into_iter().filter(|e| {
        let p = &score.parts[e.part];
        if any_solo { p.solo } else { !p.muted }
    }).collect()
}

/// Sounds that should be heard: honours mute and solo.
pub fn audible_sounds(score: &Score) -> Vec<Sound> {
    let any_solo = score.parts.iter().any(|p| p.solo);
    sounds(score)
        .into_iter()
        .filter(|s| {
            let p = &score.parts[s.part];
            if any_solo { p.solo } else { !p.muted }
        })
        .collect()
}

// ---------------------------------------------------------------- MIDI

fn vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut n = 0;
    buf[n] = (v & 0x7f) as u8;
    v >>= 7;
    while v > 0 {
        n += 1;
        buf[n] = ((v & 0x7f) as u8) | 0x80;
        v >>= 7;
    }
    for i in (0..=n).rev() {
        out.push(buf[i]);
    }
}

fn track(data: Vec<u8>) -> Vec<u8> {
    let mut out = b"MTrk".to_vec();
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(data);
    out
}

/// Standard MIDI File, format 1: a conductor track plus one track per part.
pub fn export_midi(score: &Score) -> Vec<u8> {
    let mut out = b"MThd".to_vec();
    out.extend(6u32.to_be_bytes());
    out.extend(1u16.to_be_bytes());
    out.extend(((score.parts.len() + 1) as u16).to_be_bytes());
    out.extend((PPQ as u16).to_be_bytes());

    let mut t0 = Vec::new();
    vlq(&mut t0, 0);
    t0.extend([0xff, 0x03, score.title.len().min(127) as u8]);
    t0.extend(&score.title.as_bytes()[..score.title.len().min(127)]);
    vlq(&mut t0, 0);
    let us = 60_000_000 / score.tempo.max(1);
    t0.extend([0xff, 0x51, 0x03, (us >> 16) as u8, (us >> 8) as u8, us as u8]);
    let n_measures = score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    let measure_starts = score.measure_starts(n_measures);
    let mut sigs: Vec<(u32, TimeSig)> = vec![(0, score.time)];
    sigs.extend(score.time_changes.iter().filter(|c| c.measure < n_measures).map(|c| (measure_starts[c.measure], c.time)));
    let mut last_tick = 0u32;
    for (tick, ts) in sigs {
        vlq(&mut t0, tick - last_tick);
        last_tick = tick;
        let dd = ts.den.max(1).ilog2() as u8;
        t0.extend([0xff, 0x58, 0x04, ts.num as u8, dd, 24, 8]);
    }
    vlq(&mut t0, 0);
    t0.extend([0xff, 0x2f, 0x00]);
    out.extend(track(t0));

    let all = sounds(score);
    let chans = score.midi_channels();
    for (pi, part) in score.parts.iter().enumerate() {
        let ch = chans[pi];
        let mut evs: Vec<(u32, u8, i32)> = Vec::new(); // (tick, order 0=off 1=on, midi)
        for pe in pedals(score).iter().filter(|e| e.part == pi) {
            evs.push((pe.at, 2, if pe.down { 127 } else { 0 }));
        }
        let mut vels: std::collections::HashMap<(u32, i32), u8> = Default::default();
        for s in all.iter().filter(|s| s.part == pi) {
            evs.push((s.start, 1, s.midi));
            evs.push((s.start + s.ticks, 0, s.midi));
            vels.insert((s.start, s.midi), s.velocity);
        }
        evs.sort();
        let mut d = Vec::new();
        vlq(&mut d, 0);
        let name = part.name.as_bytes();
        d.extend([0xff, 0x03, name.len().min(127) as u8]);
        d.extend(&name[..name.len().min(127)]);
        vlq(&mut d, 0);
        if !part.percussion {
            d.extend([0xc0 | ch, part.program & 0x7f]);
        } else {
            d.extend([0xff, 0x01, 0x00]); // keep the delta-time slot valid
        }
        vlq(&mut d, 0);
        d.extend([0xb0 | ch, 7, part.volume & 0x7f]);
        let mut last = 0;
        for (t, on, midi) in evs {
            let vel = vels.get(&(t, midi)).copied().unwrap_or(80) & 0x7f;
            vlq(&mut d, t - last);
            last = t;
            if on == 2 {
                d.extend([0xb0 | ch, 64, midi as u8]); // sustain pedal; `midi` carries the value
                continue;
            }
            d.extend([if on == 1 { 0x90 } else { 0x80 } | ch, midi.clamp(0, 127) as u8, if on == 1 { vel } else { 0 }]);
        }
        vlq(&mut d, 0);
        d.extend([0xff, 0x2f, 0x00]);
        out.extend(track(d));
    }
    out
}

// ------------------------------------------------------------ MusicXML

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn type_name(base: u32) -> &'static str {
    match base {
        1920 => "whole",
        960 => "half",
        480 => "quarter",
        240 => "eighth",
        120 => "16th",
        60 => "32nd",
        _ => "64th",
    }
}

fn base_and_dots(ticks: u32) -> (u32, bool) {
    let mut v = PPQ * 4;
    while v >= MIN_TICKS {
        if ticks == v {
            return (v, false);
        }
        if ticks == v + v / 2 {
            return (v, true);
        }
        v /= 2;
    }
    (ticks, false)
}

const STEPS: [&str; 7] = ["C", "D", "E", "F", "G", "A", "B"];

/// MusicXML direction that starts or stops a hairpin, octave line or pedal.
fn line_direction(kind: SpannerKind, start: bool, n: usize) -> String {
    let (placement, inner) = match kind {
        SpannerKind::Crescendo => ("below", format!("<wedge type=\"{}\" number=\"{n}\"/>", if start { "crescendo" } else { "stop" })),
        SpannerKind::Decrescendo => ("below", format!("<wedge type=\"{}\" number=\"{n}\"/>", if start { "diminuendo" } else { "stop" })),
        SpannerKind::Pedal => ("below", format!("<pedal type=\"{}\" line=\"yes\"/>", if start { "start" } else { "stop" })),
        k => {
            let size = if k.octaves().abs() == 2 { 15 } else { 8 };
            // "down": written lower than it sounds (8va); "up": written higher (8vb)
            let dir = if !start { "stop" } else if k.octaves() > 0 { "down" } else { "up" };
            ("above", format!("<octave-shift type=\"{dir}\" size=\"{size}\" number=\"{n}\"/>"))
        }
    };
    format!("      <direction placement=\"{placement}\"><direction-type>{inner}</direction-type></direction>\n")
}

/// MusicXML slur notations and hairpin directions attached to one event.
#[derive(Default, Clone)]
struct XmlMarks {
    slur: String,
    dir: String,
    /// <ornaments> content (trill mark and wavy line)
    orn: String,
}

/// Slurs and hairpins of one voice in one measure as MusicXML snippets, plus the hairpin
/// stops that fall on the closing barline.
fn xml_marks(score: &Score, part: &Part, voice: usize, mi: usize, evs: &[Event]) -> (Vec<XmlMarks>, String) {
    let mut marks = vec![XmlMarks::default(); evs.len()];
    let mut tail = String::new();
    let mlen = score.measure_len(mi);
    for (idx, sp) in part.spanners.iter().enumerate().filter(|(_, s)| s.voice as usize == voice) {
        let Some((a, b)) = crate::mscx_write::snap_spanner(score, part, sp) else { continue };
        let n = idx % 6 + 1;
        if a.measure == mi {
            if let Some(i) = evs.iter().position(|e| e.start == a.tick) {
                match sp.kind {
                    SpannerKind::Slur => marks[i].slur.push_str(&format!("<slur type=\"start\" number=\"{n}\"/>")),
                    SpannerKind::Volta => {} // written as barline endings
                    SpannerKind::Trill => {
                        let single = a.measure == b.measure && evs[i].start + evs[i].ticks == b.tick;
                        marks[i].orn.push_str(&if single { "<trill-mark/>".to_string() } else { format!("<trill-mark/><wavy-line type=\"start\" number=\"{n}\"/>") });
                    }
                    k => marks[i].dir.push_str(&line_direction(k, true, n)),
                }
            }
        }
        if b.measure == mi {
            match sp.kind {
                SpannerKind::Slur => {
                    if let Some(i) = evs.iter().position(|e| e.start == b.tick) {
                        marks[i].slur.push_str(&format!("<slur type=\"stop\" number=\"{n}\"/>"));
                    }
                }
                SpannerKind::Volta => {}
                SpannerKind::Trill => {
                    if let Some(i) = evs.iter().position(|e| e.start + e.ticks == b.tick) {
                        let same = a.measure == mi && evs.iter().position(|e| e.start == a.tick) == Some(i);
                        if !same {
                            marks[i].orn.push_str(&format!("<wavy-line type=\"stop\" number=\"{n}\"/>"));
                        }
                    }
                }
                k if b.tick >= mlen => tail.push_str(&line_direction(k, false, n)),
                k => {
                    if let Some(i) = evs.iter().position(|e| e.start == b.tick) {
                        marks[i].dir.push_str(&line_direction(k, false, n));
                    }
                }
            }
        }
    }
    (marks, tail)
}

/// Nombre MusicXML (paso, alteración) de una clase de altura, con sostenidos o bemoles según la tonalidad.
fn xml_step(pc: u8, fifths: i8) -> (&'static str, i8) {
    const SHARP: [(&str, i8); 12] = [("C", 0), ("C", 1), ("D", 0), ("D", 1), ("E", 0), ("F", 0), ("F", 1), ("G", 0), ("G", 1), ("A", 0), ("A", 1), ("B", 0)];
    const FLAT: [(&str, i8); 12] = [("C", 0), ("D", -1), ("D", 0), ("E", -1), ("E", 0), ("F", 0), ("G", -1), ("G", 0), ("A", -1), ("A", 0), ("B", -1), ("B", 0)];
    if fifths >= 0 { SHARP[pc as usize % 12] } else { FLAT[pc as usize % 12] }
}

fn xml_kind(q: Quality) -> &'static str {
    match q {
        Quality::Major => "major",
        Quality::Minor => "minor",
        Quality::Dominant7 => "dominant",
        Quality::Major7 => "major-seventh",
        Quality::Minor7 => "minor-seventh",
        Quality::Dim => "diminished",
        Quality::Dim7 => "diminished-seventh",
    }
}

/// Calidad NovaScore de un `<kind>` de MusicXML (las extensiones —novenas, sextas— se reducen a la base).
fn quality_from_kind(k: &str) -> Option<Quality> {
    Some(match k {
        "major" | "major-sixth" => Quality::Major,
        "minor" | "minor-sixth" => Quality::Minor,
        "dominant" | "dominant-ninth" | "dominant-11th" | "dominant-13th" => Quality::Dominant7,
        "major-seventh" | "major-ninth" | "major-11th" | "major-13th" => Quality::Major7,
        "minor-seventh" | "minor-ninth" | "minor-11th" | "minor-13th" => Quality::Minor7,
        "diminished" | "half-diminished" => Quality::Dim,
        "diminished-seventh" => Quality::Dim7,
        _ => return None,
    })
}

fn xml_pc(step: &str, alter: i64) -> Option<u8> {
    let base = match step {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    Some((base + alter).rem_euclid(12) as u8)
}

/// `<harmony>` de un acorde; `offset` son ticks tras el inicio del evento donde se escribe.
fn xml_harmony(c: &ChordMark, fifths: i8, offset: u32) -> String {
    let (rs, ra) = xml_step(c.root, fifths);
    let mut h = String::from("      <harmony><root>");
    let _ = write!(h, "<root-step>{rs}</root-step>");
    if ra != 0 {
        let _ = write!(h, "<root-alter>{ra}</root-alter>");
    }
    let _ = write!(h, "</root><kind text=\"{}\">{}</kind>", c.quality.suffix(), xml_kind(c.quality));
    if c.bass != c.root {
        let (bs, ba) = xml_step(c.bass, fifths);
        let _ = write!(h, "<bass><bass-step>{bs}</bass-step>");
        if ba != 0 {
            let _ = write!(h, "<bass-alter>{ba}</bass-alter>");
        }
        h.push_str("</bass>");
    }
    if offset > 0 {
        let _ = write!(h, "<offset>{offset}</offset>");
    }
    h.push_str("</harmony>
");
    h
}

pub fn export_musicxml(score: &Score) -> String {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.push_str("<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n");
    x.push_str("<score-partwise version=\"4.0\">\n");
    let _ = writeln!(x, "  <work><work-title>{}</work-title></work>", esc(&score.title));
    if !score.composer.is_empty() {
        let _ = writeln!(x, "  <identification><creator type=\"composer\">{}</creator></identification>", esc(&score.composer));
    }
    let chans = score.midi_channels();
    x.push_str("  <part-list>\n");
    for (i, p) in score.parts.iter().enumerate() {
        let _ = writeln!(
            x,
            "    <score-part id=\"P{n}\"><part-name>{}</part-name><score-instrument id=\"P{n}-I1\"><instrument-name>{}</instrument-name></score-instrument><midi-instrument id=\"P{n}-I1\"><midi-channel>{}</midi-channel><midi-program>{}</midi-program></midi-instrument></score-part>",
            esc(&p.name), esc(&p.name), chans[i] as u32 + 1, p.program as u32 + 1, n = i + 1
        );
    }
    x.push_str("  </part-list>\n");

    for (pi, part) in score.parts.iter().enumerate() {
        let _ = writeln!(x, "  <part id=\"P{}\">", pi + 1);
        for (mi, m) in part.measures.iter().enumerate() {
            let _ = writeln!(x, "    <measure number=\"{}\">", mi + 1);
            let mlen = score.measure_len(mi);
            if mi > 0 {
                if let Some(c) = score.time_changes.iter().find(|c| c.measure == mi) {
                    let _ = writeln!(x, "      <attributes><time><beats>{}</beats><beat-type>{}</beat-type></time></attributes>", c.time.num, c.time.den);
                }
            }
            if mi == 0 {
                let (sign, line) = if part.clef == Clef::Treble { ("G", 2) } else { ("F", 4) };
                let _ = writeln!(
                    x,
                    "      <attributes><divisions>{}</divisions><key><fifths>{}</fifths></key><time><beats>{}</beats><beat-type>{}</beat-type></time><clef><sign>{}</sign><line>{}</line></clef></attributes>",
                    PPQ, score.fifths, score.time.num, score.time.den, sign, line
                );
                if pi == 0 {
                    let _ = writeln!(
                        x,
                        "      <direction placement=\"above\"><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>{t}</per-minute></metronome></direction-type><sound tempo=\"{t}\"/></direction>",
                        t = score.tempo
                    );
                }
            }
            // Repeat signs and volta brackets travel as barlines.
            let voltas: Vec<(usize, usize, u8)> = part
                .spanners
                .iter()
                .filter(|s| s.kind == SpannerKind::Volta)
                .map(|s| (s.start.measure, if s.end.tick == 0 && s.end.measure > s.start.measure { s.end.measure - 1 } else { s.end.measure }, s.endings))
                .collect();
            let numbers = |mask: u8| (1..=8).filter(|n| mask & (1 << (n - 1)) != 0).map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
            let opening = voltas.iter().find(|v| v.0 == mi);
            if m.start_repeat || opening.is_some() {
                let mut b = String::from("      <barline location=\"left\">");
                if m.start_repeat {
                    b.push_str("<bar-style>heavy-light</bar-style>");
                }
                if let Some(v) = opening {
                    let _ = write!(b, "<ending number=\"{}\" type=\"start\"/>", numbers(v.2));
                }
                if m.start_repeat {
                    b.push_str("<repeat direction=\"forward\"/>");
                }
                b.push_str("</barline>\n");
                x.push_str(&b);
            }
            if pi == 0 {
                for mk in m.markers.iter().filter(|k| matches!(k, Marker::Segno | Marker::Coda)) {
                    x.push_str(&xml_nav_marker(*mk));
                }
            }
            for v in 0..m.voice_count() {
            let Some(evs) = m.voice(v) else { continue };
            if v > 0 {
                let _ = writeln!(x, "      <backup><duration>{mlen}</duration></backup>");
            }
            let prev_last = if mi > 0 { part.measures[mi - 1].voice(v).and_then(|e| e.last()) } else { None };
            let next_first = part.measures.get(mi + 1).and_then(|n| n.voice(v)).and_then(|e| e.first());
            let (xm, xm_tail) = xml_marks(score, part, v, mi, evs);
            for (ei, e) in evs.iter().enumerate() {
                if pi == 0 && v == 0 {
                    // el acorde se escribe antes del evento que suena en su tick, con desplazamiento si cae a mitad
                    for c in score.chords.iter().filter(|c| c.measure == mi && e.start <= c.tick && c.tick < e.start + e.ticks) {
                        x.push_str(&xml_harmony(c, score.fifths, c.tick - e.start));
                    }
                }
                x.push_str(&xm[ei].dir);
                if let Some(d) = e.dynamic {
                    let _ = writeln!(x, "      <direction placement=\"below\"><direction-type><dynamics><{}/></dynamics></direction-type></direction>", d.name());
                }
                let prev = if ei > 0 { evs.get(ei - 1) } else { prev_last };
                let next = evs.get(ei + 1).or(next_first);
                // Tuplet markup: <time-modification> plus start/stop notations on the group edges.
                let (tmod, tnot) = match e.tuplet {
                    Some(tp) => {
                        let last = evs.get(ei + 1).map_or(true, |n| n.tuplet != Some(tp));
                        (
                            format!("<time-modification><actual-notes>{}</actual-notes><normal-notes>{}</normal-notes></time-modification>", tp.actual, tp.normal),
                            format!("{}{}", if e.start == tp.start { "<tuplet type=\"start\"/>" } else { "" }, if last { "<tuplet type=\"stop\"/>" } else { "" }),
                        )
                    }
                    None => (String::new(), String::new()),
                };
                match &e.content {
                    Content::Rest => {
                        let full = e.start == 0 && e.ticks == mlen;
                        let (base, dot) = base_and_dots(e.notated_ticks());
                        if full {
                            let _ = writeln!(x, "      <note><rest measure=\"yes\"/><duration>{}</duration><voice>{}</voice></note>", e.ticks, v + 1);
                        } else {
                            let _ = writeln!(
                                x,
                                "      <note><rest/><duration>{}</duration><voice>{}</voice><type>{}</type>{}{}{}</note>",
                                e.ticks, v + 1, type_name(base), if dot { "<dot/>" } else { "" }, tmod,
                                if tnot.is_empty() { String::new() } else { format!("<notations>{tnot}</notations>") }
                            );
                        }
                    }
                    Content::Note { pitches, tie } => {
                        let (base, dot) = base_and_dots(e.notated_ticks());
                        for (k, p) in pitches.iter().enumerate() {
                            let tie_stop = matches!(prev.map(|v| &v.content), Some(Content::Note { pitches: pp, tie: true }) if pp.iter().any(|q| q.midi() == p.midi()));
                            let tie_start = *tie && matches!(next.map(|v| &v.content), Some(Content::Note { pitches: np, .. }) if np.iter().any(|q| q.midi() == p.midi()));
                            let mut s = String::from("      <note>");
                            if k > 0 {
                                s.push_str("<chord/>");
                            }
                            let _ = write!(s, "<pitch><step>{}</step>", STEPS[p.step as usize % 7]);
                            if p.alter != 0 {
                                let _ = write!(s, "<alter>{}</alter>", p.alter);
                            }
                            let _ = write!(s, "<octave>{}</octave></pitch><duration>{}</duration>", p.octave, e.ticks);
                            if tie_stop {
                                s.push_str("<tie type=\"stop\"/>");
                            }
                            if tie_start {
                                s.push_str("<tie type=\"start\"/>");
                            }
                            let _ = write!(s, "<voice>{}</voice><type>{}</type>{}{}", v + 1, type_name(base), if dot { "<dot/>" } else { "" }, tmod);
                            let artics: String = if k == 0 && e.articulations.iter().any(|a| !a.is_ornament()) {
                                let inner: String = e
                                    .articulations
                                    .iter()
                                    .filter(|a| !a.is_ornament())
                                    .map(|a| match a {
                                        Artic::Marcato => "<strong-accent/>".to_string(),
                                        Artic::Portato => "<detached-legato/>".to_string(),
                                        other => format!("<{}/>", other.name()),
                                    })
                                    .collect();
                                format!("<articulations>{inner}</articulations>")
                            } else {
                                String::new()
                            };
                            let signs: String = e
                                .articulations
                                .iter()
                                .map(|a| match a {
                                    Artic::Mordent => "<mordent/>",
                                    Artic::ShortTrill => "<inverted-mordent/>",
                                    Artic::Turn => "<turn/>",
                                    Artic::InvertedTurn => "<inverted-turn/>",
                                    _ => "",
                                })
                                .collect();
                            let orn = if k == 0 && (!xm[ei].orn.is_empty() || !signs.is_empty()) { format!("<ornaments>{}{signs}</ornaments>", xm[ei].orn) } else { String::new() };
                            let tup = if k == 0 { format!("{tnot}{}{orn}{artics}", xm[ei].slur) } else { String::new() };
                            if tie_stop || tie_start || !tup.is_empty() {
                                s.push_str("<notations>");
                                if tie_stop {
                                    s.push_str("<tied type=\"stop\"/>");
                                }
                                if tie_start {
                                    s.push_str("<tied type=\"start\"/>");
                                }
                                s.push_str(&tup);
                                s.push_str("</notations>");
                            }
                            s.push_str("</note>");
                            x.push_str(&s);
                            x.push('\n');
                        }
                    }
                }
            }
            x.push_str(&xm_tail);
            }
            if pi == 0 {
                for mk in m.markers.iter().filter(|k| matches!(k, Marker::ToCoda | Marker::Fine)) {
                    x.push_str(&xml_nav_marker(*mk));
                }
                if let Some(j) = m.jump {
                    let sound = if j.to_segno() { "dalsegno=\"segno\"" } else { "dacapo=\"yes\"" };
                    let _ = writeln!(x, "      <direction placement=\"above\"><direction-type><words>{}</words></direction-type><sound {sound}/></direction>", j.text());
                }
            }
            let closing = voltas.iter().find(|v| v.1 == mi);
            if m.end_repeat > 0 || closing.is_some() {
                let mut b = String::from("      <barline location=\"right\">");
                if m.end_repeat > 0 {
                    b.push_str("<bar-style>light-heavy</bar-style>");
                }
                if let Some(v) = closing {
                    let _ = write!(b, "<ending number=\"{}\" type=\"{}\"/>", numbers(v.2), if m.end_repeat > 0 { "stop" } else { "discontinue" });
                }
                if m.end_repeat > 0 {
                    if m.end_repeat > 2 {
                        let _ = write!(b, "<repeat direction=\"backward\" times=\"{}\"/>", m.end_repeat);
                    } else {
                        b.push_str("<repeat direction=\"backward\"/>");
                    }
                }
                b.push_str("</barline>\n");
                x.push_str(&b);
            }
            x.push_str("    </measure>\n");
        }
        x.push_str("  </part>\n");
    }
    x.push_str("</score-partwise>\n");
    x
}

// ----------------------------------------------------- MusicXML import

#[derive(Debug)]
pub struct ImportReport {
    pub score: Score,
    pub warnings: Vec<String>,
}

fn text_of(n: roxmltree::Node, tag: &str) -> Option<String> {
    n.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).map(|t| t.trim().to_string())
}

fn num_of(n: roxmltree::Node, tag: &str) -> Option<i64> {
    text_of(n, tag).and_then(|t| t.parse::<f64>().ok()).map(|f| f.round() as i64)
}

fn child_named<'a>(n: roxmltree::Node<'a, 'a>, tag: &str) -> Option<roxmltree::Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(tag))
}

fn snap(ticks: u32) -> u32 {
    let mut best = MIN_TICKS;
    let mut bd = u32::MAX;
    let mut v = PPQ * 4;
    while v >= MIN_TICKS {
        for c in [v, v + v / 2] {
            let d = c.abs_diff(ticks);
            if d < bd {
                bd = d;
                best = c;
            }
        }
        v /= 2;
    }
    best
}

pub(crate) struct Raw {
    pub start: u32,
    pub ticks: u32,
    pub pitches: Vec<Pitch>,
    pub rest: bool,
    pub tie: bool,
    /// (actual, normal, group start tick) when the note belongs to a tuplet.
    pub tup: Option<(u8, u8, u32)>,
    pub extras: Vec<Extra>,
    pub dynamic: Option<Dynamic>,
    pub articulations: Vec<Artic>,
}

/// Turn raw per-voice events of one staff into measures: durations are snapped to standard
/// values (tuplet events keep their exact length), voices are separated and gaps filled with rests.
pub(crate) fn build_measures(voices: &[Vec<Vec<Raw>>], max_measures: usize, lens: &[u32]) -> Vec<Measure> {
        let mut out = Vec::new();
        for mi in 0..max_measures {
          let mlen = lens.get(mi).or(lens.last()).copied().unwrap_or(1920);
          let mut built: Vec<Vec<Event>> = Vec::new();
          for vmeasures in voices {
            let raws = vmeasures.get(mi);
            let mut events: Vec<Event> = Vec::new();
            let mut pos = 0u32;
            if let Some(raws) = raws {
                let mut sorted: Vec<&Raw> = raws.iter().collect();
                sorted.sort_by_key(|r| r.start);
                for r in sorted {
                    if r.ticks == 0 || r.start < pos || r.start >= mlen {
                        continue;
                    }
                    // Tuplet notes keep their exact length when the written value is a standard one.
                    let tuplet = r.tup.filter(|&(a, n, _)| tuplet_notated(r.ticks, a, n).map_or(false, valid_duration) && r.start + r.ticks <= mlen);
                    if let Some((a, n, gs)) = tuplet {
                        let content = if r.rest { Content::Rest } else { Content::note(r.pitches.clone(), r.tie) };
                        events.push(Event { start: r.start, ticks: r.ticks, content, tuplet: Some(Tuplet { actual: a, normal: n, start: gs }), extras: r.extras.clone(), dynamic: r.dynamic, articulations: r.articulations.clone() });
                        pos = r.start + r.ticks;
                        continue;
                    }
                    let mut t = snap(r.ticks);
                    while r.start + t > mlen && t > MIN_TICKS {
                        t = split_standard(mlen - r.start).first().copied().unwrap_or(MIN_TICKS);
                    }
                    if r.start + t > mlen {
                        continue;
                    }
                    let content = if r.rest { Content::Rest } else { Content::note(r.pitches.clone(), r.tie) };
                    let mut ev = Event::new(r.start, t, content);
                    ev.extras = r.extras.clone();
                    ev.dynamic = r.dynamic;
                    ev.articulations = r.articulations.clone();
                    events.push(ev);
                    pos = r.start + t;
                }
            }
            built.push(if raws.map_or(true, |r| r.is_empty()) { Vec::new() } else { fill_rests(events, mlen) });
          }
          let mut it = built.into_iter();
          let first = it.next().filter(|e| !e.is_empty()).unwrap_or_else(|| fill_rests(Vec::new(), mlen));
          let mut extra: Vec<Vec<Event>> = it.map(|v| if v.iter().all(|e| e.content == Content::Rest) { Vec::new() } else { v }).collect();
          while extra.last().map_or(false, |v| v.is_empty()) {
              extra.pop();
          }
          out.push(Measure { events: first, voices: extra, extras: Vec::new(), start_repeat: false, end_repeat: 0, markers: Vec::new(), jump: None });
        }
    out
}

pub fn import_musicxml(text: &str) -> Result<ImportReport, String> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opts).map_err(|e| format!("XML inválido: {e}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("score-partwise") {
        return Err("Solo se admite MusicXML 'score-partwise' (sin comprimir)".into());
    }
    let mut warnings = Vec::new();
    let mut score = Score::new("Sin título", TimeSig { num: 4, den: 4 }, 0, 0);
    score.parts.clear();
    if let Some(t) = root.descendants().find(|n| n.has_tag_name("work-title") || n.has_tag_name("movement-title")).and_then(|n| n.text()) {
        score.title = t.trim().to_string();
    }
    if let Some(c) = root.descendants().find(|n| n.has_tag_name("creator") && n.attribute("type") == Some("composer")).and_then(|n| n.text()) {
        score.composer = c.trim().to_string();
    }
    let mut time_map: std::collections::BTreeMap<usize, TimeSig> = Default::default();
    let mut got_tempo = false;
    let mut dropped = 0usize;

    // Part names/programs.
    let mut infos: std::collections::HashMap<String, (String, u8, bool)> = Default::default();
    for sp in root.descendants().filter(|n| n.has_tag_name("score-part")) {
        let id = sp.attribute("id").unwrap_or("").to_string();
        let name = text_of(sp, "part-name").unwrap_or_else(|| "Instrumento".into());
        let prog = sp.descendants().find(|n| n.has_tag_name("midi-program")).and_then(|n| n.text()).and_then(|t| t.trim().parse::<i32>().ok()).unwrap_or(1);
        let chan = sp.descendants().find(|n| n.has_tag_name("midi-channel")).and_then(|n| n.text()).and_then(|t| t.trim().parse::<i32>().ok());
        infos.insert(id, (name, (prog - 1).clamp(0, 127) as u8, chan == Some(10)));
    }
    // Unpitched (drum) notes refer to an instrument whose <midi-unpitched> is the drum note + 1.
    let drum_map: std::collections::HashMap<String, i32> = root
        .descendants()
        .filter(|n| n.has_tag_name("midi-instrument"))
        .filter_map(|n| Some((n.attribute("id")?.to_string(), num_of(n, "midi-unpitched")? as i32 - 1)))
        .collect();

    // (part index, staff) -> measures of raw events and clef
    // (name, program, clef, voices -> measures -> events)
    let mut staves: Vec<(String, u8, Clef, bool, Vec<Vec<Vec<Raw>>>)> = Vec::new();
    let mut perc_staves: std::collections::BTreeSet<usize> = Default::default();
    // Slurs and hairpins per staff: (voice, kind, start, end).
    let mut staff_sp: Vec<Vec<(usize, SpannerKind, SpPos, SpPos, u8)>> = Vec::new();
    // repeat signs per staff: (measure, is start, times)
    let mut repeat_by_staff: std::collections::HashMap<usize, Vec<(usize, bool, u8)>> = Default::default();
    // Navigation found in any part: (measure, marker) and (measure, is D.S., words of the direction)
    let mut nav_marks: Vec<(usize, Marker)> = Vec::new();
    let mut nav_jumps: Vec<(usize, bool, String)> = Vec::new();
    let mut max_measures = 0usize;
    // Cifrado (`<harmony>`) de la primera parte que lo trae: (compás, tick) → acorde.
    let mut harmony_chords: Vec<ChordMark> = Vec::new();
    let mut harmony_part: Option<String> = None;
    let mut harmony_skipped = 0usize;

    for part in root.children().filter(|n| n.has_tag_name("part")) {
        let pid = part.attribute("id").unwrap_or("").to_string();
        let (pname, prog, perc) = infos.get(&pid).cloned().unwrap_or(("Instrumento".into(), 0, false));
        let mut divisions = 1.0f64;
        let mut voice_ids: std::collections::HashMap<(u32, String), usize> = Default::default();
        // (staff, voice, kind, start, end) of finished slurs/hairpins; staves are resolved after the part.
        let mut part_spans: Vec<(u32, usize, SpannerKind, SpPos, SpPos)> = Vec::new();
        let mut pending_dyn: std::collections::HashMap<u32, Dynamic> = Default::default();
        let mut trill_open: std::collections::HashMap<(u32, usize, String), (usize, i64)> = Default::default();
        let mut part_repeats: Vec<(usize, bool, u8)> = Vec::new();
        let mut volta_open: Option<(usize, u8)> = None;
        let mut part_voltas: Vec<(usize, usize, u8)> = Vec::new();
        let mut slur_open: std::collections::HashMap<(u32, usize, String), (usize, i64)> = Default::default();
        let mut wedge_open: std::collections::HashMap<(u32, String), (SpannerKind, usize, (usize, i64))> = Default::default();
        let mut clefs: std::collections::HashMap<u32, Clef> = Default::default();
        let first_staff_idx = staves.len();
        let mut staff_index: std::collections::HashMap<u32, usize> = Default::default();
        let mut mcount = 0usize;

        for (mi, meas) in part.children().filter(|n| n.has_tag_name("measure")).enumerate() {
            mcount = mi + 1;
            let mut pos: i64 = 0;
            // Open tuplet groups per (staff, voice): (actual, normal, start, count, explicit markup).
            let mut groups: std::collections::HashMap<(usize, usize), (u8, u8, i64, u32, bool)> = Default::default();
            let mut last_note: Option<(usize, usize, usize, i64)> = None; // (staff idx, voice idx, event idx, start) for chords
            for el in meas.children().filter(|n| n.is_element()) {
                match el.tag_name().name() {
                    "attributes" => {
                        if let Some(d) = num_of(el, "divisions") {
                            divisions = d.max(1) as f64;
                        }
                        if let Some(k) = el.children().find(|c| c.has_tag_name("key")) {
                            if mi == 0 && staves.len() == first_staff_idx {
                                if let Some(f) = num_of(k, "fifths") {
                                    score.fifths = f.clamp(-7, 7) as i8;
                                }
                            }
                        }
                        if let Some(t) = el.children().find(|c| c.has_tag_name("time")) {
                            if let (Some(b), Some(bt)) = (num_of(t, "beats"), num_of(t, "beat-type")) {
                                if b > 0 && b <= 32 && matches!(bt, 1 | 2 | 4 | 8 | 16) {
                                    time_map.entry(mi).or_insert(TimeSig { num: b as u32, den: bt as u32 });
                                }
                            }
                        }
                        for c in el.children().filter(|c| c.has_tag_name("clef")) {
                            let no = c.attribute("number").and_then(|v| v.parse().ok()).unwrap_or(1u32);
                            let clef = if text_of(c, "sign").as_deref() == Some("F") { Clef::Bass } else { Clef::Treble };
                            clefs.insert(no, clef);
                        }
                    }
                    "harmony" => {
                        if harmony_part.as_deref().map_or(true, |p| p == pid) {
                            let root_el = child_named(el, "root");
                            let kind_text = text_of(el, "kind");
                            let q = kind_text.as_deref().and_then(quality_from_kind);
                            let rpc = root_el.and_then(|r| xml_pc(&text_of(r, "root-step")?, text_of(r, "root-alter").and_then(|a| a.parse::<f64>().ok()).unwrap_or(0.0).round() as i64));
                            match (rpc, q) {
                                (Some(root), Some(quality)) => {
                                    harmony_part = Some(pid.clone());
                                    let bass = child_named(el, "bass").and_then(|b| xml_pc(&text_of(b, "bass-step")?, text_of(b, "bass-alter").and_then(|a| a.parse::<f64>().ok()).unwrap_or(0.0).round() as i64)).unwrap_or(root);
                                    let off = (num_of(el, "offset").unwrap_or(0) as f64 * PPQ as f64 / divisions).round() as i64;
                                    harmony_chords.push(ChordMark { measure: mi, tick: (pos + off).max(0) as u32, root, quality, bass, manual: true });
                                }
                                // «none» y los rótulos de texto no son acordes; el resto (sus, aug…) no se reconocen
                                (Some(_), None) if kind_text.as_deref().map_or(false, |k| k != "none" && k != "other") => harmony_skipped += 1,
                                _ => {}
                            }
                        }
                    }
                    "direction" | "sound" => {
                        // Dynamics apply to the next note of the staff.
                        if let Some(d) = el.descendants().filter(|n| n.has_tag_name("dynamics")).flat_map(|n| n.children().filter(|c| c.is_element())).find_map(|c| Dynamic::from_name(c.tag_name().name())) {
                            pending_dyn.insert(num_of(el, "staff").unwrap_or(1).max(1) as u32, d);
                        }
                        // Octave lines and pedal: <octave-shift type="down|up|stop" size="8|15"/>, <pedal type="start|stop|change"/>
                        for w in el.descendants().filter(|n| n.has_tag_name("octave-shift") || n.has_tag_name("pedal")) {
                            let staff = num_of(el, "staff").unwrap_or(1).max(1) as u32;
                            let vtxt = text_of(el, "voice").unwrap_or_else(|| "1".into());
                            let vi = voice_ids.get(&(staff, vtxt)).copied().unwrap_or(0);
                            let is_pedal = w.has_tag_name("pedal");
                            let key = (staff, if is_pedal { "pedal".to_string() } else { format!("o{}", w.attribute("number").unwrap_or("1")) });
                            let size15 = w.attribute("size") == Some("15");
                            let kind = if is_pedal {
                                Some(SpannerKind::Pedal)
                            } else {
                                match w.attribute("type") {
                                    Some("down") => Some(if size15 { SpannerKind::Ottava15ma } else { SpannerKind::Ottava8va }),
                                    Some("up") => Some(if size15 { SpannerKind::Ottava15mb } else { SpannerKind::Ottava8vb }),
                                    _ => None,
                                }
                            };
                            let ty = w.attribute("type");
                            // MuseScore writes pedal as resume/discontinue as well as start/stop/change.
                            let stops = matches!(ty, Some("stop") | Some("discontinue")) || (is_pedal && ty == Some("change"));
                            let starts = (is_pedal && matches!(ty, Some("start") | Some("resume") | Some("change"))) || (!is_pedal && kind.is_some());
                            if stops {
                                if let Some((k, v, s)) = wedge_open.remove(&key) {
                                    part_spans.push((staff, v, k, SpPos { measure: s.0, tick: s.1.max(0) as u32 }, SpPos { measure: mi, tick: pos.max(0) as u32 }));
                                }
                            }
                            if starts {
                                wedge_open.insert(key, (kind.unwrap(), vi, (mi, pos)));
                            }
                        }
                        // Hairpins: <wedge type="crescendo|diminuendo|stop" number="n"/>
                        for w in el.descendants().filter(|n| n.has_tag_name("wedge")) {
                            let staff = num_of(el, "staff").unwrap_or(1).max(1) as u32;
                            let vtxt = text_of(el, "voice").unwrap_or_else(|| "1".into());
                            let vi = voice_ids.get(&(staff, vtxt)).copied().unwrap_or(0);
                            let number = w.attribute("number").unwrap_or("1").to_string();
                            match w.attribute("type") {
                                Some("crescendo") => {
                                    wedge_open.insert((staff, number), (SpannerKind::Crescendo, vi, (mi, pos)));
                                }
                                Some("diminuendo") => {
                                    wedge_open.insert((staff, number), (SpannerKind::Decrescendo, vi, (mi, pos)));
                                }
                                Some("stop") => {
                                    if let Some((kind, v, s)) = wedge_open.remove(&(staff, number)) {
                                        part_spans.push((staff, v, kind, SpPos { measure: s.0, tick: s.1.max(0) as u32 }, SpPos { measure: mi, tick: pos.max(0) as u32 }));
                                    }
                                }
                                _ => {}
                            }
                        }
                        // Segno, coda, fine and the D.C./D.S. jumps are `<sound>` attributes.
                        let words = el.descendants().filter(|n| n.has_tag_name("words")).filter_map(|n| n.text()).collect::<Vec<_>>().join(" ").to_lowercase();
                        // MuseScore 4 writes `fine="yes"` on every marker and the signs as <symbol>; trust the text.
                        let symbols = el.descendants().filter(|n| n.has_tag_name("symbol")).filter_map(|n| n.text()).collect::<Vec<_>>().join(" ").to_lowercase();
                        let mut sound_segno = false;
                        let mut sound_coda = false;
                        for snd in el.descendants().filter(|n| n.has_tag_name("sound")) {
                            if snd.attribute("segno").is_some() {
                                nav_marks.push((mi, Marker::Segno));
                                sound_segno = true;
                            }
                            if snd.attribute("coda").is_some() {
                                nav_marks.push((mi, Marker::Coda));
                                sound_coda = true;
                            }
                            if snd.attribute("tocoda").is_some() || words.contains("to coda") {
                                nav_marks.push((mi, Marker::ToCoda));
                            }
                            if snd.attribute("fine").is_some() && (words.contains("fine") || (words.trim().is_empty() && symbols.trim().is_empty())) {
                                nav_marks.push((mi, Marker::Fine));
                            }
                            if snd.attribute("dacapo").map_or(false, |v| v == "yes") {
                                nav_jumps.push((mi, false, words.clone()));
                            }
                            if snd.attribute("dalsegno").is_some() {
                                nav_jumps.push((mi, true, words.clone()));
                            }
                        }
                        if !sound_segno && (el.descendants().any(|n| n.has_tag_name("segno")) || symbols.split_whitespace().any(|w| w == "segno")) {
                            nav_marks.push((mi, Marker::Segno));
                        }
                        if !sound_coda && (el.descendants().any(|n| n.has_tag_name("coda")) || symbols.split_whitespace().any(|w| w == "coda")) {
                            nav_marks.push((mi, Marker::Coda));
                        }
                        if !got_tempo {
                            let t = el
                                .descendants()
                                .find(|n| n.has_tag_name("sound") && n.attribute("tempo").is_some())
                                .and_then(|n| n.attribute("tempo").and_then(|v| v.parse::<f64>().ok()))
                                .or_else(|| el.attribute("tempo").and_then(|v| v.parse::<f64>().ok()));
                            if let Some(t) = t {
                                score.tempo = (t.round() as u32).clamp(20, 300);
                                got_tempo = true;
                            }
                        }
                    }
                    "barline" => {
                        if let Some(rep) = child_named(el, "repeat") {
                            match rep.attribute("direction") {
                                Some("forward") => part_repeats.push((mi, true, 0)),
                                Some("backward") => part_repeats.push((mi, false, rep.attribute("times").and_then(|t| t.parse::<u8>().ok()).unwrap_or(2).max(2))),
                                _ => {}
                            }
                        }
                        if let Some(en) = child_named(el, "ending") {
                            let mask = en.attribute("number").unwrap_or("1").split(',').filter_map(|n| n.trim().parse::<u32>().ok()).filter(|n| (1..=8).contains(n)).fold(0u8, |m, n| m | 1 << (n - 1));
                            match en.attribute("type") {
                                Some("start") => volta_open = Some((mi, mask)),
                                Some("stop") | Some("discontinue") => {
                                    if let Some((sm, m)) = volta_open.take() {
                                        part_voltas.push((sm, mi, m));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "backup" => pos -= (num_of(el, "duration").unwrap_or(0) as f64 * PPQ as f64 / divisions).round() as i64,
                    "forward" => pos += (num_of(el, "duration").unwrap_or(0) as f64 * PPQ as f64 / divisions).round() as i64,
                    "note" => {
                        let staff = num_of(el, "staff").unwrap_or(1).max(1) as u32;
                        let voice = text_of(el, "voice").unwrap_or_else(|| "1".into());
                        let grace = el.children().any(|c| c.has_tag_name("grace"));
                        let chord = el.children().any(|c| c.has_tag_name("chord"));
                        let dur = (num_of(el, "duration").unwrap_or(0) as f64 * PPQ as f64 / divisions).round() as i64;
                        // Voices of a staff map to our voice slots in order of appearance.
                        let vi = match voice_ids.get(&(staff, voice.clone())) {
                            Some(&v) => Some(v),
                            None => {
                                let used = voice_ids.keys().filter(|k| k.0 == staff).count();
                                if used < MAX_VOICES {
                                    voice_ids.insert((staff, voice.clone()), used);
                                    Some(used)
                                } else {
                                    None
                                }
                            }
                        };
                        let keep = vi.is_some() && !grace;
                        let vi = vi.unwrap_or(0);
                        let start = if chord { last_note.map(|l| l.3).unwrap_or(pos) } else { pos };
                        if !chord && !grace {
                            pos += dur;
                        }
                        if !keep {
                            if !grace {
                                dropped += 1;
                            }
                            continue;
                        }
                        let si = *staff_index.entry(staff).or_insert_with(|| {
                            let clef = clefs.get(&staff).copied().unwrap_or(Clef::Treble);
                            let nm = if staff > 1 { format!("{pname} ({staff})") } else { pname.clone() };
                            staves.push((nm, prog, clef, perc, Vec::new()));
                            staff_sp.push(Vec::new());
                            staves.len() - 1
                        });
                        while staves[si].4.len() <= vi {
                            staves[si].4.push(Vec::new());
                        }
                        let ms = &mut staves[si].4[vi];
                        while ms.len() <= mi {
                            ms.push(Vec::new());
                        }
                        let rest = el.children().any(|c| c.has_tag_name("rest"));
                        let tie = el.children().any(|c| c.has_tag_name("tie") && c.attribute("type") == Some("start"));
                        let mut articulations: Vec<Artic> = Vec::new();
                        for a in el.descendants().filter(|c| c.has_tag_name("articulations")).flat_map(|c| c.children().filter(|x| x.is_element())) {
                            let art = match a.tag_name().name() {
                                "strong-accent" => Some(Artic::Marcato),
                                "detached-legato" => Some(Artic::Portato),
                                other => Artic::from_name(other),
                            };
                            if let Some(art) = art {
                                if !articulations.contains(&art) {
                                    articulations.push(art);
                                }
                            }
                        }
                        for o in el.descendants().filter(|c| c.has_tag_name("ornaments")).flat_map(|c| c.children().filter(|x| x.is_element())) {
                            let art = match o.tag_name().name() {
                                "mordent" => Some(Artic::Mordent),
                                "inverted-mordent" => Some(Artic::ShortTrill),
                                "turn" => Some(Artic::Turn),
                                "inverted-turn" => Some(Artic::InvertedTurn),
                                _ => None,
                            };
                            if let Some(art) = art {
                                if !articulations.contains(&art) {
                                    articulations.push(art);
                                }
                            }
                        }
                        // Trills: <trill-mark/> with an optional <wavy-line type="start|stop"/>
                        {
                            let has_mark = el.descendants().any(|c| c.has_tag_name("trill-mark"));
                            let mut wavy_start = false;
                            for w in el.descendants().filter(|c| c.has_tag_name("wavy-line")) {
                                let key = (staff, vi, w.attribute("number").unwrap_or("1").to_string());
                                match w.attribute("type") {
                                    Some("start") => {
                                        wavy_start = true;
                                        trill_open.insert(key, (mi, start));
                                    }
                                    Some("stop") => {
                                        if let Some(o) = trill_open.remove(&key) {
                                            part_spans.push((staff, vi, SpannerKind::Trill, SpPos { measure: o.0, tick: o.1.max(0) as u32 }, SpPos { measure: mi, tick: (start + dur).max(0) as u32 }));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if has_mark && !wavy_start {
                                part_spans.push((staff, vi, SpannerKind::Trill, SpPos { measure: mi, tick: start.max(0) as u32 }, SpPos { measure: mi, tick: (start + dur).max(0) as u32 }));
                            }
                        }
                        for sl in el.descendants().filter(|c| c.has_tag_name("slur")) {
                            let key = (staff, vi, sl.attribute("number").unwrap_or("1").to_string());
                            match sl.attribute("type") {
                                Some("start") => {
                                    slur_open.insert(key, (mi, start));
                                }
                                Some("stop") => {
                                    if let Some(o) = slur_open.remove(&key) {
                                        part_spans.push((staff, vi, SpannerKind::Slur, SpPos { measure: o.0, tick: o.1.max(0) as u32 }, SpPos { measure: mi, tick: start.max(0) as u32 }));
                                    }
                                }
                                _ => {}
                            }
                        }
                        let mut pitch = el.children().find(|c| c.has_tag_name("pitch")).and_then(|p| {
                            let step = STEPS.iter().position(|s| Some(*s) == text_of(p, "step").as_deref())? as u8;
                            let alter = num_of(p, "alter").unwrap_or(0).clamp(-2, 2) as i8;
                            let octave = num_of(p, "octave")? as i8;
                            Some(Pitch { step, alter, octave })
                        });
                        if pitch.is_none() && el.children().any(|c| c.has_tag_name("unpitched")) {
                            let inst = el.children().find(|c| c.has_tag_name("instrument")).and_then(|c| c.attribute("id"));
                            if let Some(midi) = inst.and_then(|i| drum_map.get(i)) {
                                pitch = Some(Pitch::from_midi(*midi, true));
                                perc_staves.insert(si);
                            }
                        }
                        if chord {
                            if let (Some((lsi, lvi, lei, _)), Some(p)) = (last_note, pitch) {
                                if lsi == si && lvi == vi {
                                    let ev = &mut ms[mi][lei];
                                    ev.pitches.push(p);
                                    ev.tie |= tie;
                                }
                            }
                            continue;
                        }
                        let tm = el.children().find(|c| c.has_tag_name("time-modification")).and_then(|c| Some((num_of(c, "actual-notes")? as u8, num_of(c, "normal-notes")? as u8)));
                        let marks: Vec<String> = el.descendants().filter(|c| c.has_tag_name("tuplet")).filter_map(|c| c.attribute("type").map(|s| s.to_string())).collect();
                        let tup = match tm {
                            Some((a, n)) if tuplet_ratio_supported(a, n) => {
                                let key = (si, vi);
                                let explicit_start = marks.iter().any(|s| s == "start");
                                let reuse = match groups.get(&key) {
                                    Some(g) => g.0 == a && g.1 == n && !explicit_start && (g.4 || g.3 < a as u32),
                                    None => false,
                                };
                                if !reuse {
                                    groups.insert(key, (a, n, start, 0, false));
                                }
                                let g = groups.get_mut(&key).unwrap();
                                if explicit_start {
                                    g.4 = true;
                                }
                                g.3 += 1;
                                let gs = g.2.max(0) as u32;
                                if marks.iter().any(|s| s == "stop") {
                                    groups.remove(&key);
                                }
                                Some((a, n, gs))
                            }
                            _ => {
                                groups.remove(&(si, vi));
                                None
                            }
                        };
                        ms[mi].push(Raw { start: start.max(0) as u32, ticks: dur.max(0) as u32, pitches: pitch.into_iter().collect(), rest: rest || pitch.is_none(), tie, tup, extras: Vec::new(), dynamic: pending_dyn.remove(&staff), articulations });
                        last_note = Some((si, vi, ms[mi].len() - 1, start));
                    }
                    _ => {}
                }
            }
        }
        for (staff, vi, kind, a, b) in part_spans {
            if let Some(&si) = staff_index.get(&staff) {
                staff_sp[si].push((vi, kind, a, b, 0));
            }
        }
        // Repeat signs go to every staff of the part; voltas to its first staff.
        for si in first_staff_idx..staves.len() {
            repeat_by_staff.entry(si).or_default().extend(part_repeats.iter().copied());
        }
        if staves.len() > first_staff_idx {
            for &(sm, em, mask) in &part_voltas {
                staff_sp[first_staff_idx].push((0, SpannerKind::Volta, SpPos { measure: sm, tick: 0 }, SpPos { measure: em + 1, tick: 0 }, mask));
            }
        }
        // Default clef lookup now that all attributes were read.
        for (staff, si) in &staff_index {
            if let Some(c) = clefs.get(staff) {
                staves[*si].2 = *c;
            }
        }
        max_measures = max_measures.max(mcount);
    }
    if dropped > 0 {
        warnings.push(format!("{dropped} notas de voces adicionales no se importaron (se admiten hasta 4 voces por pentagrama)."));
    }
    if staves.is_empty() {
        return Err("La partitura no contiene notas".into());
    }

    for si in perc_staves {
        staves[si].3 = true;
    }
    score.time = time_map.get(&0).copied().unwrap_or(TimeSig { num: 4, den: 4 });
    let mut prev = score.time;
    for (&mi, &ts) in &time_map {
        if mi > 0 && ts != prev {
            score.time_changes.push(TimeChange { measure: mi, time: ts });
            prev = ts;
        }
    }
    harmony_chords.sort_by_key(|c| (c.measure, c.tick));
    harmony_chords.dedup_by_key(|c| (c.measure, c.tick));
    score.chords = harmony_chords;
    if harmony_skipped > 0 {
        warnings.push(format!("{harmony_skipped} acordes con una calidad no reconocida (suspendidos, aumentados…) no se importaron."));
    }
    let lens: Vec<u32> = (0..max_measures).map(|i| score.measure_len(i)).collect();
    // "al Fine" or "al Coda" comes from the words of the jump, else from the markers the score has.
    nav_marks.sort_by_key(|&(m, k)| (m, k as u8));
    nav_marks.dedup();
    nav_jumps.sort();
    nav_jumps.dedup();
    let has = |k: Marker| nav_marks.iter().any(|&(_, x)| x == k);
    let jumps: Vec<(usize, Jump)> = nav_jumps
        .iter()
        .map(|(m, ds, words)| {
            let al_fine = words.contains("fine") || (!words.contains("coda") && has(Marker::Fine) && !has(Marker::ToCoda));
            let al_coda = !words.contains("fine") && (words.contains("coda") || (has(Marker::ToCoda) && !has(Marker::Fine)));
            let j = match (*ds, al_fine, al_coda) {
                (false, true, _) => Jump::DcAlFine,
                (false, _, true) => Jump::DcAlCoda,
                (false, _, _) => Jump::Dc,
                (true, true, _) => Jump::DsAlFine,
                (true, _, true) => Jump::DsAlCoda,
                (true, _, _) => Jump::Ds,
            };
            (*m, j)
        })
        .collect();
    for (si, (name, program, clef, perc, voices)) in staves.into_iter().enumerate() {
        let mut out = build_measures(&voices, max_measures, &lens);
        for &(m, k) in &nav_marks {
            if let Some(me) = out.get_mut(m) {
                me.markers.push(k);
            }
        }
        for &(m, j) in &jumps {
            if let Some(me) = out.get_mut(m) {
                me.jump = Some(j);
            }
        }
        for &(m, is_start, times) in repeat_by_staff.get(&si).into_iter().flatten() {
            if let Some(me) = out.get_mut(m) {
                if is_start {
                    me.start_repeat = true;
                } else {
                    me.end_repeat = times;
                }
            }
        }
        let mut part = Part::new(name, clef, program, out);
        part.spanners = staff_sp[si]
            .iter()
            .filter_map(|&(vi, kind, start, mut end, endings)| {
                // A hairpin that stops on a barline belongs to the measure it closes.
                if kind != SpannerKind::Slur && end.tick == 0 && end.measure > 0 {
                    end = SpPos { measure: end.measure - 1, tick: lens[end.measure - 1] };
                }
                let ok = vi < MAX_VOICES && start < end && end.measure < part.measures.len() && end.tick <= lens.get(end.measure).copied().unwrap_or(0);
                ok.then_some(Spanner { kind, voice: vi as u8, start, end, endings })
            })
            .collect();
        part.percussion = perc;
        score.parts.push(part);
    }
    Ok(ImportReport { score, warnings })
}

// --------------------------------------------------------- MIDI import

/// Import a Standard MIDI File. Notes are quantized to 16th notes; each (track, channel)
/// becomes one part with a single voice (overlapping notes are cut at the next onset).
pub fn import_midi(bytes: &[u8]) -> Result<ImportReport, String> {
    use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
    use std::collections::BTreeMap;

    const GRID: u32 = 120;
    const MAX_MEASURES: u32 = 1000;

    let smf = Smf::parse(bytes).map_err(|e| format!("MIDI inválido: {e}"))?;
    let tpq = match smf.header.timing {
        Timing::Metrical(t) => u32::from(t.as_int()).max(1),
        Timing::Timecode(..) => return Err("MIDI con temporización SMPTE no soportado".into()),
    };
    let scale = |t: u64| -> u32 { ((t * PPQ as u64 + tpq as u64 / 2) / tpq as u64) as u32 };

    let mut warnings = Vec::new();
    let mut score = Score::new("MIDI importado", TimeSig { num: 4, den: 4 }, 0, 1);
    score.parts.clear();
    let (mut got_tempo, mut got_key) = (false, false);
    let mut ts_events: Vec<(u32, TimeSig)> = Vec::new();

    struct Stream {
        name: String,
        program: u8,
        percussion: bool,
        notes: Vec<(u32, u32, u8)>, // start, end (ticks @ PPQ), key
    }
    let mut streams: Vec<Stream> = Vec::new();
    let mut triplet_hint = false;

    for (ti, track) in smf.tracks.iter().enumerate() {
        let mut t: u64 = 0;
        let mut track_name = String::new();
        let mut by_ch: BTreeMap<u8, usize> = BTreeMap::new();
        let mut open: std::collections::HashMap<(u8, u8), Vec<u32>> = Default::default();
        for ev in track {
            t += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) if track_name.is_empty() => {
                    track_name = String::from_utf8_lossy(n).trim().to_string();
                }
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if !got_tempo => {
                    let us = us.as_int().max(1);
                    score.tempo = ((60_000_000.0 / us as f64).round() as u32).clamp(20, 300);
                    got_tempo = true;
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d, _, _)) => {
                    let den = 1u32 << d.min(5);
                    if n > 0 && n <= 32 && matches!(den, 1 | 2 | 4 | 8 | 16) {
                        ts_events.push((scale(t), TimeSig { num: n as u32, den }));
                    }
                }
                TrackEventKind::Meta(MetaMessage::KeySignature(sf, _)) if !got_key => {
                    score.fifths = sf.clamp(-7, 7);
                    got_key = true;
                }
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    let stream = |streams: &mut Vec<Stream>, by_ch: &mut BTreeMap<u8, usize>| -> usize {
                        *by_ch.entry(ch).or_insert_with(|| {
                            let name = if ch == 9 {
                                "Percusión".to_string()
                            } else if track_name.is_empty() {
                                format!("Instrumento {}", streams.len() + 1)
                            } else {
                                track_name.clone()
                            };
                            streams.push(Stream { name, program: 0, percussion: ch == 9, notes: Vec::new() });
                            streams.len() - 1
                        })
                    };
                    match message {
                        MidiMessage::ProgramChange { program } => {
                            let i = stream(&mut streams, &mut by_ch);
                            if streams[i].notes.is_empty() {
                                streams[i].program = program.as_int();
                            }
                        }
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            open.entry((ch, key.as_int())).or_default().push(scale(t));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            if let Some(start) = open.get_mut(&(ch, key.as_int())).and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) }) {
                                let i = stream(&mut streams, &mut by_ch);
                                streams[i].notes.push((start, scale(t), key.as_int()));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let _ = ti;
    }
    streams.retain(|s| !s.notes.is_empty());
    if streams.is_empty() {
        return Err("El archivo MIDI no contiene notas".into());
    }

    // Time signatures: the first sets the initial one; later ones must fall on a barline.
    ts_events.sort_by_key(|e| e.0);
    let mut cur = TimeSig { num: 4, den: 4 };
    let mut skip = 0;
    if ts_events.first().map_or(false, |e| e.0 == 0) {
        cur = ts_events[0].1;
        skip = 1;
    }
    score.time = cur;
    let (mut bound, mut bar) = (0u32, 0usize);
    let mut misaligned = 0;
    for &(tick, ts) in &ts_events[skip..] {
        while bound + cur.measure_ticks() <= tick {
            bound += cur.measure_ticks();
            bar += 1;
        }
        if ts == cur {
            continue;
        }
        if bound == tick {
            score.time_changes.push(TimeChange { measure: bar, time: ts });
            cur = ts;
        } else {
            misaligned += 1;
        }
    }
    if misaligned > 0 {
        warnings.push(format!("{misaligned} cambio(s) de compás no coinciden con una barra de compás y se ignoraron."));
    }
    let sharps = score.fifths >= 0;
    let q = |t: u32| ((t + GRID / 2) / GRID) * GRID;
    let mlen = score.time.measure_ticks();
    let limit = MAX_MEASURES * mlen;
    let mut truncated = false;

    for s in &streams {
        let mut onsets: BTreeMap<u32, Vec<(u8, u32)>> = BTreeMap::new();
        for &(st, en, key) in &s.notes {
            let qs = q(st);
            let qe = q(en).max(qs + GRID);
            if (st % GRID != 0 && (st % GRID).abs_diff(GRID / 2) < 20) || (en % GRID != 0 && (en % GRID).abs_diff(GRID / 2) < 20) {
                triplet_hint = true;
            }
            if qs >= limit {
                truncated = true;
                continue;
            }
            onsets.entry(qs).or_default().push((key, qe.min(limit) - qs));
        }
        let mean = s.notes.iter().map(|n| n.2 as u32).sum::<u32>() / s.notes.len() as u32;
        let clef = if mean < 57 && !s.percussion { Clef::Bass } else { Clef::Treble };
        let mut part = Part::new(s.name.clone(), clef, s.program, vec![Measure::empty(mlen)]);
        part.percussion = s.percussion;
        score.parts.push(part);
        let pi = score.parts.len() - 1;
        let starts: Vec<u32> = onsets.keys().copied().collect();
        for (i, st) in starts.iter().enumerate() {
            let group = &onsets[st];
            let mut dur = group.iter().map(|g| g.1).min().unwrap_or(GRID);
            if let Some(next) = starts.get(i + 1) {
                dur = dur.min(next - st);
            }
            let pitches: Vec<Pitch> = group.iter().map(|g| Pitch::from_midi(g.0 as i32, sharps)).collect();
            score.put_abs(pi, *st, dur.max(GRID), &pitches).map_err(|e| e.to_string())?;
        }
    }
    let max_len = score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(1);
    let score_lens: Vec<u32> = (0..max_len).map(|i| score.measure_len(i)).collect();
    for p in &mut score.parts {
        while p.measures.len() < max_len {
            let l = score_lens.get(p.measures.len()).copied().unwrap_or(mlen);
            p.measures.push(Measure::empty(l));
        }
    }
    if truncated {
        warnings.push(format!("El archivo se recortó a {MAX_MEASURES} compases."));
    }
    if triplet_hint {
        warnings.push("Se detectaron figuras que no encajan en la cuadrícula de semicorcheas (¿tresillos?); se cuantizaron.".into());
    }
    warnings.push("Las notas se cuantizaron a semicorcheas; la percusión (canal 10) se importa como pentagrama de percusión.".into());
    Ok(ImportReport { score, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_export_import_roundtrip() {
        let mut s = Score::new("x", TimeSig { num: 3, den: 4 }, 0, 2);
        s.tempo = 80;
        let p = |step, octave| Pitch { step, alter: 0, octave };
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 4), p(2, 4)]).unwrap();
        s.put_note(0, 0, 0, 480, 240, vec![p(4, 4)]).unwrap();
        s.put_note(0, 0, 0, 1200, 480, vec![p(5, 4)]).unwrap(); // tied across the barline
        s.parts[0].program = 40;
        let r = import_midi(&export_midi(&s)).unwrap();
        assert_eq!(r.score.tempo, 80);
        assert_eq!(r.score.time, s.time);
        assert_eq!(r.score.parts.len(), 1);
        assert_eq!(r.score.parts[0].program, 40);
        assert_eq!(sounds(&r.score), sounds(&s));
    }

    #[test]
    fn percussion_part_roundtrips_through_midi_and_musicxml() {
        let mut s = Score::new("d", TimeSig { num: 4, den: 4 }, 0, 1);
        let drum = s.add_part("Bateria", Clef::Treble, 0, true);
        let p = |m: i32| Pitch::from_midi(m, true);
        s.put_note(drum, 0, 0, 0, 480, vec![p(36)]).unwrap(); // kick
        s.put_note(drum, 0, 0, 480, 480, vec![p(42)]).unwrap(); // closed hi-hat
        assert_eq!(s.midi_channels(), vec![0, 9]);
        assert!(sounds(&s).iter().all(|x| x.channel == if x.part == 1 { 9 } else { 0 }));

        let midi = export_midi(&s);
        assert!(midi.windows(2).any(|w| w == [0x99, 36]));
        let r = import_midi(&midi).unwrap();
        assert!(r.score.parts[0].percussion && r.score.parts[0].name == "Percusión");
        assert_eq!(sounds(&r.score).iter().map(|x| x.midi).collect::<Vec<_>>(), vec![36, 42]);

        let x = import_musicxml(&export_musicxml(&s)).unwrap();
        assert_eq!(x.score.parts.len(), 2);
        assert!(x.score.parts[1].percussion && !x.score.parts[0].percussion);
    }

    #[test]
    fn midi_import_rejects_garbage() {
        assert!(import_midi(b"nope").is_err());
    }


    fn p(step: u8, alter: i8, octave: i8) -> Pitch {
        Pitch { step, alter, octave }
    }

    fn sample() -> Score {
        let mut s = Score::new("Prueba & <ok>", TimeSig { num: 3, den: 4 }, 2, 3);
        s.composer = "Yo".into();
        s.tempo = 90;
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 0, 4), p(2, 0, 4), p(4, 0, 4)]).unwrap();
        s.put_note(0, 0, 0, 480, 720, vec![p(3, 1, 4)]).unwrap();
        s.put_note(0, 0, 0, 1200, 480, vec![p(1, 0, 5)]).unwrap(); // crosses into measure 1, tied
        s
    }

    #[test]
    fn ties_are_merged_in_sounds() {
        let s = sample();
        let snd = sounds(&s);
        let d5: Vec<_> = snd.iter().filter(|x| x.midi == 74).collect();
        assert_eq!(d5.len(), 1);
        assert_eq!(d5[0].ticks, 480);
        assert_eq!(snd.len(), 5);
    }

    #[test]
    fn midi_header_and_notes() {
        let b = export_midi(&sample());
        assert_eq!(&b[0..4], b"MThd");
        assert_eq!(u16::from_be_bytes([b[10], b[11]]), 2); // conductor + 1 part
        let note_ons = b.windows(1).count(); // sanity: non-empty
        assert!(note_ons > 20);
        let on = b.iter().enumerate().filter(|(i, &v)| v == 0x90 && *i > 40).count();
        assert!(on >= 5);
    }

    #[test]
    fn musicxml_roundtrip() {
        let s = sample();
        let xml = export_musicxml(&s);
        assert!(xml.contains("&amp;"));
        let r = import_musicxml(&xml).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.score, s);
    }

    #[test]
    fn two_voices_sound_and_roundtrip() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("v", TimeSig { num: 4, den: 4 }, 0, 2);
        s.put_note(0, 0, 0, 0, 480, vec![p(4, 5)]).unwrap();
        s.put_note(0, 0, 0, 480, 960, vec![p(2, 5)]).unwrap();
        s.put_note(0, 1, 0, 0, 1920, vec![p(0, 4)]).unwrap(); // voice 2: whole note
        s.put_note(0, 1, 1, 960, 960, vec![p(4, 3)]).unwrap(); // voice 2 in measure 2, half rest first
        let snd = sounds(&s);
        assert_eq!(snd.len(), 4);
        assert!(snd.iter().any(|x| x.midi == 60 && x.ticks == 1920));
        let xml = export_musicxml(&s);
        assert!(xml.contains("<backup>") && xml.contains("<voice>2</voice>"));
        let r = import_musicxml(&xml).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.score, s);
    }

    #[test]
    fn tuplets_roundtrip_through_musicxml_and_sound_correctly() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 2);
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 5)]).unwrap();
        s.make_tuplet(0, 0, 0, 0, 3, 2).unwrap(); // eighth-note triplet
        s.put_note(0, 0, 0, 160, 240, vec![p(1, 5)]).unwrap();
        s.put_note(0, 0, 0, 320, 240, vec![p(2, 5)]).unwrap();
        s.put_note(0, 0, 0, 480, 480, vec![p(3, 5)]).unwrap();
        s.make_tuplet(0, 0, 0, 480, 5, 4).unwrap(); // quintuplet of sixteenths
        let snd = sounds(&s);
        assert_eq!(&snd.iter().take(3).map(|x| (x.start, x.ticks)).collect::<Vec<_>>(), &[(0, 160), (160, 160), (320, 160)]);
        let xml = export_musicxml(&s);
        assert!(xml.contains("<actual-notes>3</actual-notes>") && xml.contains("<tuplet type=\"start\"/>") && xml.contains("<tuplet type=\"stop\"/>"));
        assert!(xml.contains("<type>eighth</type>") && xml.contains("<type>16th</type>"));
        let r = import_musicxml(&xml).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.score, s);
    }

    #[test]
    fn septuplets_roundtrip_through_musicxml() {
        let mut s = Score::new("sept", TimeSig { num: 4, den: 4 }, 0, 2);
        s.put_note(0, 0, 0, 0, 960, vec![p(0, 0, 5)]).unwrap();
        s.make_tuplet(0, 0, 0, 0, 7, 4).unwrap();
        for i in 0..7u32 {
            let start = s.parts[0].measures[0].events[i as usize].start;
            s.put_note(0, 0, 0, start, 240, vec![p((i % 7) as u8, 0, 5)]).unwrap();
        }
        let xml = export_musicxml(&s);
        assert!(xml.contains("<actual-notes>7</actual-notes><normal-notes>4</normal-notes>"));
        let back = import_musicxml(&xml).unwrap().score;
        assert_eq!(back.parts[0].measures[0].events, s.parts[0].measures[0].events);
        // playing time: the group lasts exactly a half note
        let snd = sounds(&s);
        assert_eq!(snd.iter().filter(|x| x.start < 960).count(), 7);
        assert_eq!(snd.iter().filter(|x| x.start < 960).map(|x| x.ticks).sum::<u32>(), 960);
    }

    #[test]
    fn import_groups_tuplets_without_explicit_markup() {
        // Two consecutive triplets (divisions=3, so a triplet eighth lasts 1) with no <tuplet> elements.
        let note = |d: &str| format!("<note><pitch><step>{d}</step><octave>4</octave></pitch><duration>1</duration><time-modification><actual-notes>3</actual-notes><normal-notes>2</normal-notes></time-modification></note>");
        let notes: String = ["C", "D", "E", "F", "G", "A"].iter().map(|d| note(d)).collect();
        let rest = "<note><rest/><duration>6</duration></note>";
        let xml = format!("<score-partwise><part-list><score-part id=\"P1\"><part-name>x</part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\"><attributes><divisions>3</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>{notes}{rest}</measure></part></score-partwise>");
        let r = import_musicxml(&xml).unwrap();
        let ev = &r.score.parts[0].measures[0].events;
        let starts: Vec<_> = ev.iter().filter_map(|e| e.tuplet.map(|t| t.start)).collect();
        assert_eq!(starts, vec![0, 0, 0, 480, 480, 480]);
        assert!(ev.iter().take(6).all(|e| e.ticks == 160 && e.notated_ticks() == 240));
    }

    fn changing_meter_score() -> Score {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("m", TimeSig { num: 4, den: 4 }, 0, 4);
        s.set_time_at(2, TimeSig { num: 3, den: 4 });
        s.put_note(0, 0, 1, 0, 1920, vec![p(0, 4)]).unwrap();
        s.put_note(0, 0, 2, 0, 1440, vec![p(2, 4)]).unwrap(); // dotted half, whole 3/4 measure
        s.put_note(0, 0, 3, 480, 480, vec![p(4, 4)]).unwrap();
        s
    }

    #[test]
    fn time_changes_in_sounds_midi_and_musicxml() {
        let s = changing_meter_score();
        // absolute positions follow the real measure lengths: 1920*2 = 3840 starts the 3/4 part
        let snd = sounds(&s);
        assert_eq!(snd.iter().map(|x| (x.start, x.ticks)).collect::<Vec<_>>(), vec![(1920, 1920), (3840, 1440), (5280 + 480, 480)]);
        // MIDI: two time signature meta events, the second at tick 3840
        let midi = export_midi(&s);
        assert_eq!(midi.windows(3).filter(|w| w == &[0xff, 0x58, 0x04]).count(), 2);
        let r = import_midi(&midi).unwrap();
        assert_eq!(r.score.time_changes, s.time_changes);
        assert_eq!(sounds(&r.score), sounds(&s));
        // MusicXML roundtrip keeps the change and the notes
        let xml = export_musicxml(&s);
        assert_eq!(xml.matches("<beats>").count(), 2);
        let x = import_musicxml(&xml).unwrap();
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        assert_eq!(x.score, s);
    }

    #[test]
    fn slurs_and_hairpins_roundtrip_through_musicxml() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("e", TimeSig { num: 4, den: 4 }, 0, 3);
        for (i, st) in [0u32, 480, 960, 1440].into_iter().enumerate() {
            s.put_note(0, 0, 0, st, 480, vec![p(i as u8, 5)]).unwrap();
        }
        s.put_note(0, 0, 1, 0, 1920, vec![p(2, 5)]).unwrap();
        let sp = |kind, a: (usize, u32), b: (usize, u32)| Spanner { kind, voice: 0, start: SpPos { measure: a.0, tick: a.1 }, end: SpPos { measure: b.0, tick: b.1 }, endings: 0 };
        for x in [
            sp(SpannerKind::Slur, (0, 0), (0, 960)),
            sp(SpannerKind::Slur, (0, 1440), (1, 0)), // across the barline
            sp(SpannerKind::Crescendo, (0, 480), (0, 1440)),
            sp(SpannerKind::Decrescendo, (0, 1440), (0, 1920)), // up to the barline
        ] {
            s.add_spanner(0, x).unwrap();
        }
        let xml = export_musicxml(&s);
        assert_eq!(xml.matches("<slur type=").count(), 4);
        assert_eq!(xml.matches("<wedge type=").count(), 4);
        let r = import_musicxml(&xml).unwrap();
        let mut want = s.parts[0].spanners.clone();
        let mut got = r.score.parts[0].spanners.clone();
        want.sort_by_key(|x| (x.start, x.end));
        got.sort_by_key(|x| (x.start, x.end));
        assert_eq!(got, want);
        assert_eq!(r.score.parts[0].measures, s.parts[0].measures);
    }

    #[test]
    fn dynamics_and_articulations_drive_velocity_and_length() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("d", TimeSig { num: 4, den: 4 }, 0, 3);
        for m in 0..2 {
            for (i, st) in [0u32, 480, 960, 1440].into_iter().enumerate() {
                s.put_note(0, 0, m, st, 480, vec![p(i as u8, 4)]).unwrap();
            }
        }
        s.set_dynamic(0, 0, 0, 0, Some(Dynamic::P)).unwrap();
        s.set_dynamic(0, 0, 1, 0, Some(Dynamic::F)).unwrap();
        s.toggle_articulation(0, 0, SpPos { measure: 0, tick: 480 }, SpPos { measure: 0, tick: 480 }, Artic::Staccato).unwrap();
        s.toggle_articulation(0, 0, SpPos { measure: 0, tick: 960 }, SpPos { measure: 0, tick: 960 }, Artic::Accent).unwrap();
        let snd = sounds(&s);
        assert_eq!(snd[0].velocity, 49); // p
        assert_eq!((snd[1].ticks, snd[1].velocity), (240, 49)); // staccato halves the length
        assert_eq!((snd[2].ticks, snd[2].velocity), (480, 69)); // accent adds 20
        assert_eq!(snd[4].velocity, 96); // f from the next measure on
        // a crescendo over measure 0 ramps towards the next marking (f = 96)
        s.add_spanner(0, Spanner { kind: SpannerKind::Crescendo, voice: 0, start: SpPos { measure: 0, tick: 0 }, end: SpPos { measure: 0, tick: 1920 }, endings: 0 }).unwrap();
        let ramp = sounds(&s);
        assert_eq!(ramp[0].velocity, 49);
        assert!(ramp[1].velocity > 49 && ramp[3].velocity > ramp[1].velocity && ramp[3].velocity < 96);
        // the MIDI file carries the velocities
        let midi = export_midi(&s);
        assert!(midi.windows(3).any(|w| w[0] == 0x90 && w[2] == 96));
    }

    #[test]
    fn dynamics_and_articulations_roundtrip_through_musicxml() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("x", TimeSig { num: 4, den: 4 }, 0, 1);
        s.put_note(0, 0, 0, 0, 960, vec![p(0, 5)]).unwrap();
        s.put_note(0, 0, 0, 960, 960, vec![p(1, 5)]).unwrap();
        s.set_dynamic(0, 0, 0, 0, Some(Dynamic::Mf)).unwrap();
        s.set_dynamic(0, 0, 0, 960, Some(Dynamic::Sfz)).unwrap();
        s.toggle_articulation(0, 0, SpPos { measure: 0, tick: 0 }, SpPos { measure: 0, tick: 0 }, Artic::Staccato).unwrap();
        s.toggle_articulation(0, 0, SpPos { measure: 0, tick: 0 }, SpPos { measure: 0, tick: 960 }, Artic::Marcato).unwrap();
        let xml = export_musicxml(&s);
        assert!(xml.contains("<mf/>") && xml.contains("<sfz/>") && xml.contains("<strong-accent/>") && xml.contains("<staccato/>"));
        let r = import_musicxml(&xml).unwrap();
        assert_eq!(r.score.parts[0].measures, s.parts[0].measures);
    }

    #[test]
    fn octave_lines_and_pedal_roundtrip_through_musicxml() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("o", TimeSig { num: 4, den: 4 }, 0, 2);
        s.put_note(0, 0, 0, 0, 1920, vec![p(0, 6)]).unwrap();
        s.put_note(0, 0, 1, 0, 1920, vec![p(0, 2)]).unwrap();
        let sp = |kind, a: (usize, u32), b: (usize, u32)| Spanner { kind, voice: 0, start: SpPos { measure: a.0, tick: a.1 }, end: SpPos { measure: b.0, tick: b.1 }, endings: 0 };
        for x in [
            sp(SpannerKind::Ottava15ma, (0, 0), (0, 1920)),
            sp(SpannerKind::Ottava8vb, (1, 0), (1, 1920)),
            sp(SpannerKind::Pedal, (0, 0), (1, 1920)),
        ] {
            s.add_spanner(0, x).unwrap();
        }
        let xml = export_musicxml(&s);
        assert!(xml.contains("<octave-shift type=\"down\" size=\"15\"") && xml.contains("<octave-shift type=\"up\" size=\"8\"") && xml.contains("<pedal type=\"start\""));
        let r = import_musicxml(&xml).unwrap();
        let mut got = r.score.parts[0].spanners.clone();
        let mut want = s.parts[0].spanners.clone();
        got.sort_by_key(|x| (x.start, x.end));
        want.sort_by_key(|x| (x.start, x.end));
        assert_eq!(got, want);
        assert_eq!(r.score.parts[0].measures, s.parts[0].measures); // MusicXML pitches are the sounding ones
    }

    #[test]
    fn markers_and_jumps_roundtrip_through_musicxml() {
        let mut s = Score::new("nav", TimeSig { num: 4, den: 4 }, 0, 6);
        s.add_part("Bajo", Clef::Bass, 0, false);
        for m in 0..6 {
            s.put_note(0, 0, m, 0, 1920, vec![Pitch { step: (m % 7) as u8, alter: 0, octave: 5 }]).unwrap();
        }
        s.toggle_marker(1, Marker::Segno).unwrap();
        s.toggle_marker(2, Marker::ToCoda).unwrap();
        s.toggle_marker(5, Marker::Coda).unwrap();
        s.set_jump(4, Some(Jump::DsAlCoda)).unwrap();
        let xml = export_musicxml(&s);
        assert!(xml.contains("<segno/>") && xml.contains("<coda/>") && xml.contains("tocoda=\"coda\"") && xml.contains("dalsegno=\"segno\""));
        assert_eq!(xml.matches("<segno/>").count(), 1, "written once, in the first part");
        let back = import_musicxml(&xml).unwrap().score;
        for part in &back.parts {
            assert_eq!(part.measures.iter().map(|m| (m.markers.clone(), m.jump)).collect::<Vec<_>>(), s.parts[0].measures.iter().map(|m| (m.markers.clone(), m.jump)).collect::<Vec<_>>());
        }
        assert_eq!(back.playback_order(), s.playback_order());
        // al Fine and the plain forms are told apart by the words and the markers
        for j in [Jump::Dc, Jump::DcAlFine, Jump::DcAlCoda, Jump::Ds, Jump::DsAlFine, Jump::DsAlCoda] {
            let mut t = Score::new("j", TimeSig { num: 4, den: 4 }, 0, 3);
            t.toggle_marker(0, Marker::Segno).unwrap();
            t.toggle_marker(1, Marker::Fine).unwrap();
            t.toggle_marker(1, Marker::ToCoda).unwrap();
            t.set_jump(2, Some(j)).unwrap();
            assert_eq!(import_musicxml(&export_musicxml(&t)).unwrap().score.parts[0].measures[2].jump, Some(j));
        }
    }

    #[test]
    fn repeats_voltas_and_trills_roundtrip_through_musicxml() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("r", TimeSig { num: 4, den: 4 }, 0, 5);
        for m in 0..5 {
            s.put_note(0, 0, m, 0, 1920, vec![p(m as u8 % 7, 5)]).unwrap();
        }
        s.toggle_repeat(0, true).unwrap();
        s.toggle_repeat(2, false).unwrap();
        s.parts[0].measures[4].end_repeat = 3;
        let sp = |kind, a: (usize, u32), b: (usize, u32), endings| Spanner { kind, voice: 0, start: SpPos { measure: a.0, tick: a.1 }, end: SpPos { measure: b.0, tick: b.1 }, endings };
        for x in [
            sp(SpannerKind::Volta, (2, 0), (2, 1920), 0b01),
            sp(SpannerKind::Volta, (3, 0), (3, 1920), 0b10),
            sp(SpannerKind::Trill, (0, 0), (1, 1920), 0), // over two measures
            sp(SpannerKind::Trill, (4, 0), (4, 1920), 0), // a single note
        ] {
            s.add_spanner(0, x).unwrap();
        }
        let xml = export_musicxml(&s);
        assert!(xml.contains("<repeat direction=\"forward\"/>") && xml.contains("<repeat direction=\"backward\"/>") && xml.contains("times=\"3\""));
        assert!(xml.contains("<ending number=\"1\" type=\"start\"/>") && xml.contains("type=\"stop\"") && xml.contains("type=\"discontinue\""));
        let r = import_musicxml(&xml).unwrap().score;
        let mut got = r.parts[0].spanners.clone();
        let mut want = s.parts[0].spanners.clone();
        got.sort_by_key(|x| (x.start, x.end));
        want.sort_by_key(|x| (x.start, x.end));
        assert_eq!(got, want);
        let flags = |s: &Score| s.parts[0].measures.iter().map(|m| (m.start_repeat, m.end_repeat)).collect::<Vec<_>>();
        assert_eq!(flags(&r), flags(&s));
        assert_eq!(r.playback_order(), s.playback_order());
    }

    #[test]
    fn mordents_and_turns_play_their_notes_and_roundtrip_through_musicxml() {
        let mut s = Score::new("orn", TimeSig { num: 4, den: 4 }, 0, 2);
        let at = |t: u32| SpPos { measure: 0, tick: t };
        for (i, (a, step)) in [(Artic::Mordent, 4u8), (Artic::ShortTrill, 4), (Artic::Turn, 4), (Artic::InvertedTurn, 4)].into_iter().enumerate() {
            s.put_note(0, 0, 0, (i as u32) * 480, 480, vec![p(step, 0, 5)]).unwrap(); // G5
            s.toggle_articulation(0, 0, at(i as u32 * 480), at(i as u32 * 480), a).unwrap();
        }
        s.put_note(0, 0, 1, 0, 60, vec![p(4, 0, 5)]).unwrap(); // too short to hold a mordent
        s.toggle_articulation(0, 0, SpPos { measure: 1, tick: 0 }, SpPos { measure: 1, tick: 0 }, Artic::Mordent).unwrap();
        let sounds = sounds(&s);
        let of = |from: u32, to: u32| sounds.iter().filter(|x| x.start >= from && x.start < to).map(|x| (x.midi, x.ticks)).collect::<Vec<_>>();
        let g = 79; // G5; the scale of C has A above (81) and F below (77)
        assert_eq!(of(0, 480), vec![(g, 60), (77, 60), (g, 360)]); // mordent: lower neighbour
        assert_eq!(of(480, 960), vec![(g, 60), (81, 60), (g, 360)]); // upper mordent
        assert_eq!(of(960, 1440), vec![(81, 90), (g, 90), (77, 90), (g, 210)]); // turn
        assert_eq!(of(1440, 1920), vec![(77, 90), (g, 90), (81, 90), (g, 210)]); // inverted turn
        assert_eq!(of(1920, 2400), vec![(g, 60)]); // too short: plain note
        let xml = export_musicxml(&s);
        assert!(xml.contains("<mordent/>") && xml.contains("<inverted-mordent/>") && xml.contains("<turn/>") && xml.contains("<inverted-turn/>"));
        let back = import_musicxml(&xml).unwrap().score;
        let arts = |s: &Score| s.parts[0].measures.iter().flat_map(|m| m.events.iter().map(|e| e.articulations.clone())).collect::<Vec<_>>();
        assert_eq!(arts(&back), arts(&s));
    }

    #[test]
    fn trill_alternates_with_the_scale_note_above() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 2, 1); // D major
        s.put_note(0, 0, 0, 0, 480, vec![p(2, 5)]).unwrap(); // E5 -> F#5 (a semitone is not in D major: F# is)
        s.put_note(0, 0, 0, 480, 480, vec![p(1, 5)]).unwrap(); // D5, not trilled
        s.add_spanner(0, Spanner { kind: SpannerKind::Trill, voice: 0, start: SpPos { measure: 0, tick: 0 }, end: SpPos { measure: 0, tick: 480 }, endings: 0 }).unwrap();
        let snd = sounds(&s);
        assert_eq!(snd.len(), 8 + 1); // eight 32nds and the plain D
        assert!(snd[..8].iter().enumerate().all(|(i, x)| x.midi == if i % 2 == 0 { 76 } else { 78 } && x.ticks == 60 && x.start == i as u32 * 60));
        assert_eq!(snd[8].midi, 74);
    }

    #[test]
    fn import_rejects_garbage() {
        assert!(import_musicxml("hola").is_err());
        assert!(import_musicxml("<score-timewise/>").is_err());
    }
}
