//! NovaScore core: score model, editing operations and undo/redo.
//! Licensed under GPL-3.0-or-later.

pub mod beats;
pub mod io;
pub mod mscx;
pub mod mscx_write;
pub mod quantize;

use serde::{Deserialize, Serialize};

/// Ticks per quarter note.
pub const PPQ: u32 = 480;
/// Smallest supported grid unit (64th note = 30 ticks).
const MIN_TICKS: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pitch {
    /// 0=C .. 6=B
    pub step: u8,
    /// -2..=2 (double flat .. double sharp)
    pub alter: i8,
    pub octave: i8,
}

impl Pitch {
    pub fn midi(&self) -> i32 {
        const SEMI: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
        (self.octave as i32 + 1) * 12 + SEMI[self.step as usize % 7] + self.alter as i32
    }

    /// Spell a MIDI number, preferring sharps (`sharps = true`) or flats.
    pub fn from_midi(midi: i32, sharps: bool) -> Pitch {
        const SHARP: [(u8, i8); 12] = [
            (0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (3, 0),
            (3, 1), (4, 0), (4, 1), (5, 0), (5, 1), (6, 0),
        ];
        const FLAT: [(u8, i8); 12] = [
            (0, 0), (1, -1), (1, 0), (2, -1), (2, 0), (3, 0),
            (4, -1), (4, 0), (5, -1), (5, 0), (6, -1), (6, 0),
        ];
        let pc = midi.rem_euclid(12) as usize;
        let (step, alter) = if sharps { SHARP[pc] } else { FLAT[pc] };
        Pitch { step, alter, octave: (midi.div_euclid(12) - 1) as i8 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    /// One or more simultaneous pitches (a chord). `tie` ties to the next event
    /// for every pitch that also appears there.
    Note { pitches: Vec<Pitch>, tie: bool },
    Rest,
}

impl Content {
    pub fn note(mut pitches: Vec<Pitch>, tie: bool) -> Content {
        pitches.sort_by_key(|p| (p.midi(), p.step));
        pitches.dedup_by_key(|p| p.midi());
        Content::Note { pitches, tie }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Start tick inside the measure.
    pub start: u32,
    /// Sounding duration. Inside a tuplet this is shorter than the written value.
    pub ticks: u32,
    #[serde(flatten)]
    pub content: Content,
    /// Set when the event belongs to a tuplet group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuplet: Option<Tuplet>,
    /// MuseScore markup that NovaScore does not understand (dynamics, articulations, lyrics...),
    /// kept verbatim so it survives a round trip through `.mscz`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extras: Vec<Extra>,
    /// Dynamic marking that starts at this event (it holds until the next one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<Dynamic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub articulations: Vec<Artic>,
}

/// Dynamic markings, with the MIDI velocity MuseScore uses for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dynamic {
    Ppp,
    Pp,
    P,
    Mp,
    Mf,
    F,
    Ff,
    Fff,
    Fp,
    Sf,
    Sfz,
    Rfz,
}

impl Dynamic {
    pub const ALL: [Dynamic; 12] = [Dynamic::Ppp, Dynamic::Pp, Dynamic::P, Dynamic::Mp, Dynamic::Mf, Dynamic::F, Dynamic::Ff, Dynamic::Fff, Dynamic::Fp, Dynamic::Sf, Dynamic::Sfz, Dynamic::Rfz];

    pub fn name(self) -> &'static str {
        match self {
            Dynamic::Ppp => "ppp",
            Dynamic::Pp => "pp",
            Dynamic::P => "p",
            Dynamic::Mp => "mp",
            Dynamic::Mf => "mf",
            Dynamic::F => "f",
            Dynamic::Ff => "ff",
            Dynamic::Fff => "fff",
            Dynamic::Fp => "fp",
            Dynamic::Sf => "sf",
            Dynamic::Sfz => "sfz",
            Dynamic::Rfz => "rfz",
        }
    }

    pub fn from_name(s: &str) -> Option<Dynamic> {
        Dynamic::ALL.into_iter().find(|d| d.name() == s.trim())
    }

    /// MIDI velocity the marking asks for.
    pub fn velocity(self) -> u8 {
        match self {
            Dynamic::Ppp => 16,
            Dynamic::Pp => 33,
            Dynamic::P => 49,
            Dynamic::Mp => 64,
            Dynamic::Mf => 80,
            Dynamic::F => 96,
            Dynamic::Ff => 112,
            Dynamic::Fff => 126,
            Dynamic::Fp => 96,
            Dynamic::Sf | Dynamic::Sfz | Dynamic::Rfz => 112,
        }
    }
}

/// Articulation signs on a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Artic {
    Staccato,
    Staccatissimo,
    Tenuto,
    Accent,
    Marcato,
    /// Tenuto with staccato (louré).
    Portato,
}

impl Artic {
    pub fn name(self) -> &'static str {
        match self {
            Artic::Staccato => "staccato",
            Artic::Staccatissimo => "staccatissimo",
            Artic::Tenuto => "tenuto",
            Artic::Accent => "accent",
            Artic::Marcato => "marcato",
            Artic::Portato => "portato",
        }
    }

    pub fn from_name(s: &str) -> Option<Artic> {
        [Artic::Staccato, Artic::Staccatissimo, Artic::Tenuto, Artic::Accent, Artic::Marcato, Artic::Portato].into_iter().find(|a| a.name() == s.trim())
    }

    /// Share of the written length that sounds.
    pub fn gate(self) -> f64 {
        match self {
            Artic::Staccato => 0.5,
            Artic::Staccatissimo => 0.33,
            Artic::Portato => 0.75,
            _ => 1.0,
        }
    }

    /// Extra MIDI velocity.
    pub fn velocity_boost(self) -> i32 {
        match self {
            Artic::Accent => 20,
            Artic::Marcato => 30,
            _ => 0,
        }
    }
}

/// Where an [`Extra`] fragment goes when the score is written back to MuseScore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtraPlace {
    /// Element that precedes the chord or rest in its voice (dynamic, staff text...).
    Before,
    /// Child of the chord or rest element (articulation, lyrics, fermata...).
    Inside,
    /// Element that follows the last chord of the voice.
    After,
    /// Child of the note with this MIDI pitch (fingering, accidental style...).
    Note(i32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extra {
    pub place: ExtraPlace,
    pub xml: String,
}

/// A tuplet group: `actual` notes are played in the time of `normal` (3:2 is a triplet).
/// All events of a group carry the same value; `start` is the first tick of the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tuplet {
    pub actual: u8,
    pub normal: u8,
    pub start: u32,
}

/// Ratios whose tick values stay whole numbers at 480 ticks per quarter.
pub fn tuplet_ratio_supported(actual: u8, normal: u8) -> bool {
    matches!((actual, normal), (2, 3) | (3, 2) | (4, 3) | (5, 4) | (6, 4))
}

impl Event {
    pub fn new(start: u32, ticks: u32, content: Content) -> Event {
        Event { start, ticks, content, tuplet: None, extras: Vec::new(), dynamic: None, articulations: Vec::new() }
    }

    /// Written (notated) duration: the sounding duration scaled back by the tuplet ratio.
    pub fn notated_ticks(&self) -> u32 {
        match self.tuplet {
            Some(t) => self.ticks * t.actual as u32 / t.normal as u32,
            None => self.ticks,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measure {
    /// Voice 1.
    pub events: Vec<Event>,
    /// Voices 2-4 (index 0 is voice 2). An empty list means the voice has no content here.
    #[serde(default)]
    pub voices: Vec<Vec<Event>>,
    /// MuseScore measure-level markup (repeats, markers, breaks...) kept verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extras: Vec<String>,
    /// Repeat sign at the start of the measure.
    #[serde(default, skip_serializing_if = "is_false")]
    pub start_repeat: bool,
    /// Repeat sign at the end: the number of times the section is played (0 = none, 2 = once repeated).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub end_repeat: u8,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// Maximum number of voices per staff.
pub const MAX_VOICES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSig {
    pub num: u32,
    pub den: u32,
}

impl TimeSig {
    pub fn measure_ticks(&self) -> u32 {
        self.num * PPQ * 4 / self.den
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Clef {
    Treble,
    Bass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub clef: Clef,
    /// MIDI General MIDI program (0-127).
    pub program: u8,
    /// Mixer volume (MIDI CC7, 0-127).
    #[serde(default = "default_volume")]
    pub volume: u8,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    /// Percussion staff: notes are General MIDI drum numbers played on channel 10.
    #[serde(default)]
    pub percussion: bool,
    /// Where this part came from in a MuseScore file (instrument definition, staff settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<MsOrigin>,
    /// Slurs and hairpins, anchored to positions in time so they stay put while notes change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spanners: Vec<Spanner>,
    pub measures: Vec<Measure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpannerKind {
    Slur,
    Crescendo,
    Decrescendo,
    /// 8va: notes sound an octave above what is written.
    Ottava8va,
    /// 8vb: notes sound an octave below what is written.
    Ottava8vb,
    Ottava15ma,
    Ottava15mb,
    /// Piano pedal down until the end of the span.
    Pedal,
    /// Trill sign with a wavy line.
    Trill,
    /// First/second ending bracket ("casilla"); see `Spanner::endings`.
    Volta,
}

impl SpannerKind {
    /// Octaves between the sounding and the written pitch (8va = +1, 15mb = -2).
    pub fn octaves(self) -> i32 {
        match self {
            SpannerKind::Ottava8va => 1,
            SpannerKind::Ottava8vb => -1,
            SpannerKind::Ottava15ma => 2,
            SpannerKind::Ottava15mb => -2,
            _ => 0,
        }
    }

    /// Hairpins, octave lines and pedal run over a stretch of time (a slur joins two notes).
    pub fn is_line(self) -> bool {
        self != SpannerKind::Slur
    }
}

/// A point in time: measure index and tick inside the measure (the tick may equal the measure
/// length, meaning the closing barline).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SpPos {
    pub measure: usize,
    pub tick: u32,
}

/// A slur between two notes of one voice, or a hairpin over a stretch of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spanner {
    pub kind: SpannerKind,
    pub voice: u8,
    pub start: SpPos,
    pub end: SpPos,
    /// For a volta: bit n-1 set means the bracket is played on pass n (1 = first ending).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub endings: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

/// MuseScore data of an imported part, reused when the score is saved as `.mscz` so that
/// instruments, brackets and sound settings are not lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MsOrigin {
    /// Parts imported from the same MuseScore part (a piano's two staves) share the number.
    pub group: u32,
    /// The `<Instrument>` element as written by MuseScore 4.
    pub instrument_xml: String,
    /// Contents of this staff's `<Staff>` element inside `<Part>`.
    pub staff_xml: String,
    // The values below are used to notice that the user changed the instrument afterwards.
    pub name: String,
    pub program: u8,
    pub clef: Clef,
    pub percussion: bool,
}

fn default_volume() -> u8 {
    100
}

impl Part {
    fn measure_events_mut(&mut self, m: usize, voice: usize) -> &mut Vec<Event> {
        self.measures[m].voice_mut(voice).expect("voice checked by caller")
    }

    pub fn new(name: impl Into<String>, clef: Clef, program: u8, measures: Vec<Measure>) -> Part {
        Part { name: name.into(), clef, program, volume: default_volume(), muted: false, solo: false, percussion: false, ms: None, spanners: Vec::new(), measures }
    }
}

/// Partial update of a part (mixer and instrument settings).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartUpdate {
    pub name: Option<String>,
    pub clef: Option<Clef>,
    pub program: Option<u8>,
    pub volume: Option<u8>,
    pub muted: Option<bool>,
    pub solo: Option<bool>,
    pub percussion: Option<bool>,
}

/// A time signature that starts at `measure` (0-based) and lasts until the next change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeChange {
    pub measure: usize,
    pub time: TimeSig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score {
    pub title: String,
    pub composer: String,
    /// Initial time signature.
    pub time: TimeSig,
    /// Later changes, sorted by measure and never equal to the signature in force before them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time_changes: Vec<TimeChange>,
    /// Key signature as number of sharps (positive) or flats (negative).
    pub fifths: i8,
    pub tempo: u32,
    pub parts: Vec<Part>,
    /// Style settings (`score_style.mss`) of the MuseScore file this score came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_style: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    OutOfRange,
    BadDuration,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::OutOfRange => write!(f, "position out of range"),
            EditError::BadDuration => write!(f, "invalid duration"),
        }
    }
}

/// Split a span into standard (power of two or dotted) rest values, largest first.
fn split_standard(mut ticks: u32) -> Vec<u32> {
    let mut out = Vec::new();
    while ticks >= MIN_TICKS {
        let mut best = MIN_TICKS;
        let mut v = PPQ * 4;
        while v >= MIN_TICKS {
            if v <= ticks {
                best = v;
                break;
            }
            v /= 2;
        }
        // Use a dotted value when it fits exactly.
        let dotted = best + best / 2;
        if dotted == ticks && best / 2 >= MIN_TICKS {
            best = dotted;
        }
        out.push(best);
        ticks -= best;
    }
    out
}

fn valid_duration(ticks: u32) -> bool {
    let mut v = PPQ * 4;
    while v >= MIN_TICKS {
        if ticks == v || (v / 2 >= MIN_TICKS && ticks == v + v / 2) {
            return true;
        }
        v /= 2;
    }
    false
}

/// Rests filling `[from, to)` inside a tuplet group, split into standard written values.
fn tuplet_rests(from: u32, to: u32, t: Tuplet) -> Result<Vec<Event>, EditError> {
    if to <= from {
        return Ok(Vec::new());
    }
    let notated = (to - from) * t.actual as u32 / t.normal as u32;
    let mut out = Vec::new();
    let mut pos = from;
    for piece in split_standard(notated) {
        if (piece * t.normal as u32) % t.actual as u32 != 0 {
            return Err(EditError::BadDuration);
        }
        let real = piece * t.normal as u32 / t.actual as u32;
        out.push(Event { start: pos, ticks: real, content: Content::Rest, tuplet: Some(t), extras: Vec::new(), dynamic: None, articulations: Vec::new() });
        pos += real;
    }
    Ok(out)
}

impl Measure {
    pub fn empty(len: u32) -> Measure {
        Measure { events: fill_rests(Vec::new(), len), voices: Vec::new(), extras: Vec::new(), start_repeat: false, end_repeat: 0 }
    }

    /// Events of a voice (0-based), or `None` if the voice has no content in this measure.
    pub fn voice(&self, v: usize) -> Option<&Vec<Event>> {
        if v == 0 {
            Some(&self.events)
        } else {
            self.voices.get(v - 1).filter(|e| !e.is_empty())
        }
    }

    pub fn voice_mut(&mut self, v: usize) -> Option<&mut Vec<Event>> {
        if v == 0 {
            Some(&mut self.events)
        } else {
            self.voices.get_mut(v - 1).filter(|e| !e.is_empty())
        }
    }

    /// Like `voice_mut`, but creates the voice filled with rests if needed.
    pub fn voice_create(&mut self, v: usize, len: u32) -> &mut Vec<Event> {
        if v == 0 {
            return &mut self.events;
        }
        while self.voices.len() < v {
            self.voices.push(Vec::new());
        }
        if self.voices[v - 1].is_empty() {
            self.voices[v - 1] = fill_rests(Vec::new(), len);
        }
        &mut self.voices[v - 1]
    }

    /// Number of voices that have content in this measure (at least 1).
    pub fn voice_count(&self) -> usize {
        1 + self.voices.iter().rposition(|v| !v.is_empty()).map_or(0, |i| i + 1)
    }
}

/// Sort events and fill any gaps with rests.
fn fill_rests(mut events: Vec<Event>, len: u32) -> Vec<Event> {
    events.sort_by_key(|e| e.start);
    let mut out = Vec::new();
    let mut pos = 0;
    let push_gap = |out: &mut Vec<Event>, from: u32, to: u32| {
        let mut p = from;
        for t in split_standard(to - from) {
            out.push(Event::new(p, t, Content::Rest));
            p += t;
        }
    };
    for e in events {
        if e.start > pos {
            push_gap(&mut out, pos, e.start);
        }
        pos = e.start + e.ticks;
        out.push(e);
    }
    if pos < len {
        push_gap(&mut out, pos, len);
    }
    out
}

impl Score {
    pub fn new(title: &str, time: TimeSig, fifths: i8, measures: usize) -> Score {
        let len = time.measure_ticks();
        Score {
            title: title.to_string(),
            composer: String::new(),
            time,
            time_changes: Vec::new(),
            fifths,
            tempo: 100,
            ms_style: None,
            parts: vec![Part::new("Piano", Clef::Treble, 0, (0..measures).map(|_| Measure::empty(len)).collect())],
        }
    }

    fn measure_mut(&mut self, part: usize, m: usize) -> Result<&mut Measure, EditError> {
        self.parts.get_mut(part).and_then(|p| p.measures.get_mut(m)).ok_or(EditError::OutOfRange)
    }

    /// Events of an existing voice (error when that voice has no content in the measure).
    fn voice_events_mut(&mut self, part: usize, voice: usize, m: usize) -> Result<&mut Vec<Event>, EditError> {
        self.measure_mut(part, m)?.voice_mut(voice).ok_or(EditError::OutOfRange)
    }

    /// Place an event inside one measure of a voice, replacing overlapped material. The voice is
    /// created (filled with rests) when it does not exist yet. The duration is clamped to the end
    /// of the measure. Returns the end tick.
    pub fn put(&mut self, part: usize, voice: usize, m: usize, start: u32, ticks: u32, content: Content) -> Result<u32, EditError> {
        if !valid_duration(ticks) {
            return Err(EditError::BadDuration);
        }
        if voice >= MAX_VOICES {
            return Err(EditError::OutOfRange);
        }
        let len = self.measure_len(m);
        if start >= len {
            return Err(EditError::OutOfRange);
        }
        let meas = self.measure_mut(part, m)?;
        let events = meas.voice_create(voice, len);
        // Overwriting part of a tuplet group removes the whole group; the new event then starts
        // where the group started.
        let mut start = start;
        let (mut end, mut lo, mut hi);
        loop {
            end = (start + ticks).min(len);
            lo = start;
            hi = end;
            for e in events.iter().filter(|e| e.tuplet.is_some() && e.start < end && e.start + e.ticks > start) {
                let g = e.tuplet.unwrap();
                for o in events.iter().filter(|o| o.tuplet == Some(g)) {
                    lo = lo.min(o.start);
                    hi = hi.max(o.start + o.ticks);
                }
            }
            if lo < start {
                start = lo;
            } else {
                break;
            }
        }
        let mut kept: Vec<Event> = events.iter().filter(|e| e.start + e.ticks <= lo || e.start >= hi).cloned().collect();
        let mut new_ticks = end - start;
        if !valid_duration(new_ticks) {
            new_ticks = split_standard(new_ticks).first().copied().unwrap_or(ticks);
        }
        kept.push(Event::new(start, new_ticks, content));
        *events = fill_rests(kept, len);
        Ok(start + new_ticks)
    }

    /// Enter a note/chord that may cross barlines: it is split into tied notes and
    /// measures are appended when needed. Returns the next cursor position `(measure, tick)`.
    pub fn put_note(&mut self, part: usize, voice: usize, m: usize, start: u32, ticks: u32, pitches: Vec<Pitch>) -> Result<(usize, u32), EditError> {
        if !valid_duration(ticks) || pitches.is_empty() {
            return Err(EditError::BadDuration);
        }
        let len = self.measure_len(m);
        if start >= len || voice >= MAX_VOICES || self.parts.get(part).map_or(true, |p| m >= p.measures.len()) {
            return Err(EditError::OutOfRange);
        }
        if let Some(t) = self.tuplet_at(part, voice, m, start) {
            return self.enter_in_tuplet(part, voice, m, start, ticks, Content::note(pitches, false), t);
        }
        let (mut cm, mut cs, mut remaining) = (m, start, ticks);
        loop {
            if cm >= self.parts[part].measures.len() {
                self.add_measures(1);
            }
            let seg = remaining.min(self.measure_len(cm) - cs);
            let pieces = if valid_duration(seg) { vec![seg] } else { split_standard(seg) };
            let n = pieces.len();
            for (i, piece) in pieces.into_iter().enumerate() {
                let last = remaining == seg && i + 1 == n;
                self.put(part, voice, cm, cs, piece, Content::note(pitches.clone(), !last))?;
                cs += piece;
            }
            remaining -= seg;
            if remaining == 0 {
                break;
            }
            cm += 1;
            cs = 0;
        }
        if cs >= self.measure_len(cm) {
            if cm + 1 >= self.parts[part].measures.len() {
                self.add_measures(1);
            }
            Ok((cm + 1, 0))
        } else {
            Ok((cm, cs))
        }
    }

    fn tuplet_at(&self, part: usize, voice: usize, m: usize, start: u32) -> Option<Tuplet> {
        let meas = self.parts.get(part)?.measures.get(m)?;
        meas.voice(voice)?.iter().find(|e| e.start <= start && start < e.start + e.ticks)?.tuplet
    }

    /// Enter a note or rest of written value `notated` at a position inside a tuplet group.
    #[allow(clippy::too_many_arguments)]
    fn enter_in_tuplet(&mut self, part: usize, voice: usize, m: usize, start: u32, notated: u32, content: Content, t: Tuplet) -> Result<(usize, u32), EditError> {
        if !valid_duration(notated) || (notated * t.normal as u32) % t.actual as u32 != 0 {
            return Err(EditError::BadDuration);
        }
        let real = notated * t.normal as u32 / t.actual as u32;
        let events = self.voice_events_mut(part, voice, m)?;
        let group: Vec<&Event> = events.iter().filter(|e| e.tuplet == Some(t)).collect();
        let (gs, ge) = (group.iter().map(|e| e.start).min().unwrap_or(0), group.iter().map(|e| e.start + e.ticks).max().unwrap_or(0));
        let end = start + real;
        if start < gs || end > ge {
            return Err(EditError::BadDuration); // does not fit inside the group
        }
        let (mut lo, mut hi) = (start, end);
        for e in group.iter().filter(|e| e.start < end && e.start + e.ticks > start) {
            lo = lo.min(e.start);
            hi = hi.max(e.start + e.ticks);
        }
        let mut next: Vec<Event> = events.iter().filter(|e| e.tuplet != Some(t) || e.start + e.ticks <= lo || e.start >= hi).cloned().collect();
        next.extend(tuplet_rests(lo, start, t)?);
        next.push(Event { start, ticks: real, content, tuplet: Some(t), extras: Vec::new(), dynamic: None, articulations: Vec::new() });
        next.extend(tuplet_rests(end, hi, t)?);
        next.sort_by_key(|e| e.start);
        *events = next;
        Ok(self.next_position(m, end))
    }

    /// Cursor position after something ended at tick `end` of measure `m`; appends a measure
    /// when the end falls on the last barline.
    fn next_position(&mut self, m: usize, end: u32) -> (usize, u32) {
        if end < self.measure_len(m) {
            return (m, end);
        }
        while self.parts.iter().any(|p| p.measures.len() <= m + 1) {
            self.add_measures(1);
        }
        (m + 1, 0)
    }

    /// Enter a rest, which may fall inside a tuplet. Returns the next cursor position.
    pub fn put_rest(&mut self, part: usize, voice: usize, m: usize, start: u32, ticks: u32) -> Result<(usize, u32), EditError> {
        if let Some(t) = self.tuplet_at(part, voice, m, start) {
            return self.enter_in_tuplet(part, voice, m, start, ticks, Content::Rest, t);
        }
        let end = self.put(part, voice, m, start, ticks, Content::Rest)?;
        Ok(self.next_position(m, end))
    }

    /// Turn the event at `start` into a tuplet group: it is split into `actual` equal slots that
    /// together last as long as the original event; the first slot keeps its content.
    pub fn make_tuplet(&mut self, part: usize, voice: usize, m: usize, start: u32, actual: u8, normal: u8) -> Result<(), EditError> {
        if !tuplet_ratio_supported(actual, normal) {
            return Err(EditError::BadDuration);
        }
        let events = self.voice_events_mut(part, voice, m)?;
        let idx = events.iter().position(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        let e = events[idx].clone();
        if e.tuplet.is_some() {
            return Err(EditError::OutOfRange);
        }
        let span = e.ticks;
        if span % actual as u32 != 0 || span % normal as u32 != 0 || !valid_duration(span / normal as u32) {
            return Err(EditError::BadDuration);
        }
        let each = span / actual as u32;
        let t = Tuplet { actual, normal, start };
        let slots: Vec<Event> = (0..actual as u32)
            .map(|i| Event { start: start + i * each, ticks: each, content: if i == 0 { e.content.clone() } else { Content::Rest }, tuplet: Some(t), extras: if i == 0 { e.extras.clone() } else { Vec::new() }, dynamic: if i == 0 { e.dynamic } else { None }, articulations: if i == 0 { e.articulations.clone() } else { Vec::new() } })
            .collect();
        events.splice(idx..=idx, slots);
        Ok(())
    }

    /// Dissolve the tuplet group containing the event at `start` back into a single event.
    pub fn clear_tuplet(&mut self, part: usize, voice: usize, m: usize, start: u32) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let t = events.iter().find(|e| e.start <= start && start < e.start + e.ticks).and_then(|e| e.tuplet).ok_or(EditError::OutOfRange)?;
        let group: Vec<Event> = events.iter().filter(|e| e.tuplet == Some(t)).cloned().collect();
        let total: u32 = group.iter().map(|e| e.ticks).sum();
        let first_start = group[0].start;
        let content = group[0].content.clone();
        events.retain(|e| e.tuplet != Some(t));
        let pieces = if valid_duration(total) { vec![total] } else { split_standard(total) };
        let mut pos = first_start;
        for (i, p) in pieces.into_iter().enumerate() {
            events.push(Event::new(pos, p, if i == 0 { content.clone() } else { Content::Rest }));
            pos += p;
        }
        events.sort_by_key(|e| e.start);
        Ok(())
    }

    /// Turn an event into a rest (merging neighbouring rests). An extra voice that ends up
    /// containing only rests is removed from the measure.
    pub fn delete_event(&mut self, part: usize, voice: usize, m: usize, start: u32) -> Result<(), EditError> {
        let len = self.measure_len(m);
        let events = self.voice_events_mut(part, voice, m)?;
        let idx = events.iter().position(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        events[idx].content = Content::Rest;
        events[idx].extras.retain(|x| matches!(x.place, ExtraPlace::Before | ExtraPlace::After));
        events[idx].articulations.clear();
        // Merge adjacent plain rests back into standard values (tuplet events keep their slots).
        let mut merged: Vec<Event> = Vec::new();
        // A run of merged rests keeps the dynamic and markup of its first event.
        let mut run: Option<(u32, u32, Option<Dynamic>, Vec<Extra>)> = None;
        let flush = |merged: &mut Vec<Event>, (s, t, dynamic, extras): (u32, u32, Option<Dynamic>, Vec<Extra>)| {
            let mut p = s;
            for (i, v) in split_standard(t).into_iter().enumerate() {
                let mut ev = Event::new(p, v, Content::Rest);
                if i == 0 {
                    ev.dynamic = dynamic;
                    ev.extras = extras.clone();
                }
                merged.push(ev);
                p += v;
            }
        };
        for e in events.drain(..) {
            if e.content == Content::Rest && e.tuplet.is_none() {
                run = Some(match run {
                    Some((s, t, d, x)) => (s, t + e.ticks, d.or(e.dynamic), x),
                    None => (e.start, e.ticks, e.dynamic, e.extras.clone()),
                });
            } else {
                if let Some(r) = run.take() {
                    flush(&mut merged, r);
                }
                merged.push(e);
            }
        }
        if let Some(r) = run {
            flush(&mut merged, r);
        }
        *events = fill_rests(merged, len);
        if voice > 0 && events.iter().all(|e| e.content == Content::Rest) {
            self.measure_mut(part, m)?.voices[voice - 1].clear();
        }
        Ok(())
    }

    /// Add a pitch to an existing note (making a chord).
    pub fn add_pitch(&mut self, part: usize, voice: usize, m: usize, start: u32, pitch: Pitch) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        match &e.content {
            Content::Note { pitches, tie } => {
                e.content = Content::note([pitches.clone(), vec![pitch]].concat(), *tie);
                e.extras.retain(|x| !matches!(x.place, ExtraPlace::Note(_)));
                Ok(())
            }
            Content::Rest => Err(EditError::OutOfRange),
        }
    }

    /// Remove one pitch from a chord; removing the last pitch turns the event into a rest.
    pub fn remove_pitch(&mut self, part: usize, voice: usize, m: usize, start: u32, midi: i32) -> Result<(), EditError> {
        {
            let events = self.voice_events_mut(part, voice, m)?;
            let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
            if let Content::Note { pitches, .. } = &mut e.content {
                if pitches.len() > 1 {
                    pitches.retain(|p| p.midi() != midi);
                    return Ok(());
                }
            } else {
                return Err(EditError::OutOfRange);
            }
        }
        self.delete_event(part, voice, m, start)
    }

    pub fn toggle_tie(&mut self, part: usize, voice: usize, m: usize, start: u32) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        match &mut e.content {
            Content::Note { tie, .. } => {
                *tie = !*tie;
                Ok(())
            }
            Content::Rest => Err(EditError::OutOfRange),
        }
    }

    pub fn transpose_event(&mut self, part: usize, voice: usize, m: usize, start: u32, semitones: i32) -> Result<(), EditError> {
        let sharps = if semitones != 0 { semitones > 0 } else { self.fifths >= 0 };
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        if let Content::Note { pitches, .. } = &mut e.content {
            for p in pitches.iter_mut() {
                *p = Pitch::from_midi(p.midi() + semitones, sharps);
            }
            e.extras.retain(|x| !matches!(x.place, ExtraPlace::Note(_)));
            Ok(())
        } else {
            Err(EditError::OutOfRange)
        }
    }

    /// Place a chord at an absolute tick of one part, for any duration that is a multiple of
    /// the smallest grid unit. The span is split into standard values joined by ties and
    /// the part grows as needed. Used by importers.
    pub fn put_abs(&mut self, part: usize, abs: u32, ticks: u32, pitches: &[Pitch]) -> Result<(), EditError> {
        self.put_abs_voice(part, 0, abs, ticks, pitches)
    }

    /// Like [`Score::put_abs`] but in any voice (0-3); the voice is created where needed.
    pub fn put_abs_voice(&mut self, part: usize, voice: usize, abs: u32, ticks: u32, pitches: &[Pitch]) -> Result<(), EditError> {
        if part >= self.parts.len() || voice >= MAX_VOICES || ticks < MIN_TICKS || pitches.is_empty() {
            return Err(EditError::OutOfRange);
        }
        let (mut pos, mut remaining) = (abs, ticks);
        while remaining >= MIN_TICKS {
            let (m, start) = self.locate(pos);
            let len = self.measure_len(m);
            while self.parts[part].measures.len() <= m {
                let l = self.measure_len(self.parts[part].measures.len());
                self.parts[part].measures.push(Measure::empty(l));
            }
            let seg = remaining.min(len - start);
            let pieces = split_standard(seg);
            if pieces.is_empty() {
                break;
            }
            let n = pieces.len();
            let mut p = start;
            for (i, piece) in pieces.into_iter().enumerate() {
                let last = remaining == seg && i + 1 == n;
                self.put(part, voice, m, p, piece, Content::note(pitches.to_vec(), !last))?;
                p += piece;
            }
            let used = p - start;
            pos += used;
            remaining -= used;
        }
        Ok(())
    }

    /// MIDI channel (0-15) of every part: percussion uses channel 10 (index 9); the other parts
    /// take the remaining channels in order (the last one is shared when there are more than 15).
    pub fn midi_channels(&self) -> Vec<u8> {
        let mut next = 0u8;
        self.parts
            .iter()
            .map(|p| {
                if p.percussion {
                    return 9;
                }
                if next == 9 {
                    next = 10;
                }
                let c = next.min(15);
                next = next.saturating_add(1);
                c
            })
            .collect()
    }

    /// Set (or clear with `None`) the dynamic marking that starts at an event. Setting the
    /// marking the event already has removes it.
    pub fn set_dynamic(&mut self, part: usize, voice: usize, m: usize, start: u32, d: Option<Dynamic>) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        e.dynamic = if e.dynamic == d { None } else { d };
        Ok(())
    }

    /// Toggle an articulation on the notes of a voice between two positions (inclusive): it is
    /// added to all of them, or removed from all when they already have it. Returns true if added.
    pub fn toggle_articulation(&mut self, part: usize, voice: usize, from: SpPos, to: SpPos, a: Artic) -> Result<bool, EditError> {
        let p = self.parts.get_mut(part).ok_or(EditError::OutOfRange)?;
        let mut targets: Vec<(usize, usize)> = Vec::new();
        for m in from.measure..=to.measure.min(p.measures.len().saturating_sub(1)) {
            let Some(evs) = p.measures[m].voice(voice) else { continue };
            for (i, e) in evs.iter().enumerate() {
                let pos = SpPos { measure: m, tick: e.start };
                if pos >= from && pos <= to && matches!(e.content, Content::Note { .. }) {
                    targets.push((m, i));
                }
            }
        }
        if targets.is_empty() {
            return Err(EditError::OutOfRange);
        }
        let all_have = targets.iter().all(|&(m, i)| p.measure_events_mut(m, voice)[i].articulations.contains(&a));
        for &(m, i) in &targets {
            let e = &mut p.measure_events_mut(m, voice)[i];
            if all_have {
                e.articulations.retain(|x| *x != a);
            } else if !e.articulations.contains(&a) {
                e.articulations.push(a);
            }
        }
        Ok(!all_have)
    }

    /// Toggle the repeat sign at the start or end of a measure (applies to every part).
    pub fn toggle_repeat(&mut self, measure: usize, start: bool) -> Result<(), EditError> {
        if self.parts.iter().any(|p| measure >= p.measures.len()) {
            return Err(EditError::OutOfRange);
        }
        for p in &mut self.parts {
            let m = &mut p.measures[measure];
            if start {
                m.start_repeat = !m.start_repeat;
            } else {
                m.end_repeat = if m.end_repeat == 0 { 2 } else { 0 };
            }
        }
        Ok(())
    }

    /// Measures in the order they are played: repeats are unfolded and each volta is only played
    /// on its pass. A measure index can appear several times.
    pub fn playback_order(&self) -> Vec<usize> {
        let n = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        let Some(first) = self.parts.first() else { return Vec::new() };
        // voltas as (first measure, last measure, endings mask)
        let mut voltas: Vec<(usize, usize, u8)> = Vec::new();
        for p in &self.parts {
            for s in p.spanners.iter().filter(|s| s.kind == SpannerKind::Volta) {
                let last = if s.end.tick == 0 && s.end.measure > s.start.measure { s.end.measure - 1 } else { s.end.measure };
                voltas.push((s.start.measure, last, s.endings));
            }
        }
        let mut order = Vec::new();
        let (mut i, mut pass, mut section_start) = (0usize, 1usize, 0usize);
        let mut jumped = false; // just came back to the start of a repeated section
        let mut guard = 0usize;
        while i < n && guard < 100_000 {
            guard += 1;
            // a volta that is not for this pass is skipped
            if let Some(&(_, last, mask)) = voltas.iter().find(|v| v.0 <= i && i <= v.1) {
                if mask != 0 && pass <= 8 && mask & (1 << (pass - 1)) == 0 {
                    i = last + 1;
                    continue;
                }
            }
            let m = first.measures.get(i);
            if m.map_or(false, |m| m.start_repeat) && !jumped {
                section_start = i;
                pass = 1;
            }
            jumped = false;
            order.push(i);
            let end = m.map_or(0, |m| m.end_repeat) as usize;
            if end > 0 {
                if pass < end {
                    pass += 1;
                    i = section_start;
                    jumped = true;
                    continue;
                }
                pass = 1;
                section_start = i + 1;
            }
            i += 1;
        }
        order
    }

    /// Slurs and hairpins of a part (see `Spanner`).
    pub fn add_spanner(&mut self, part: usize, sp: Spanner) -> Result<(), EditError> {
        let n = self.parts.get(part).ok_or(EditError::OutOfRange)?.measures.len();
        if sp.voice as usize >= MAX_VOICES || sp.start >= sp.end || sp.end.measure >= n || sp.start.measure >= n {
            return Err(EditError::OutOfRange);
        }
        if sp.end.tick > self.measure_len(sp.end.measure) {
            return Err(EditError::OutOfRange);
        }
        let list = &mut self.parts[part].spanners;
        if !list.contains(&sp) {
            list.push(sp);
        }
        Ok(())
    }

    /// Add the spanner, or remove it if an identical one exists. Returns true when added.
    pub fn toggle_spanner(&mut self, part: usize, sp: Spanner) -> Result<bool, EditError> {
        let list = &mut self.parts.get_mut(part).ok_or(EditError::OutOfRange)?.spanners;
        if let Some(i) = list.iter().position(|x| *x == sp) {
            list.remove(i);
            return Ok(false);
        }
        self.add_spanner(part, sp)?;
        Ok(true)
    }

    /// Remove every spanner of the voice that overlaps `[from, to]`. Returns how many.
    pub fn remove_spanners_in(&mut self, part: usize, voice: usize, from: SpPos, to: SpPos) -> Result<usize, EditError> {
        let list = &mut self.parts.get_mut(part).ok_or(EditError::OutOfRange)?.spanners;
        let before = list.len();
        list.retain(|s| !(s.voice as usize == voice && s.start <= to && s.end >= from));
        Ok(before - list.len())
    }

    /// Append an empty instrument with as many measures as the score has. Returns its index.
    pub fn add_part(&mut self, name: &str, clef: Clef, program: u8, percussion: bool) -> usize {
        let n = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(1);
        let measures: Vec<Measure> = (0..n).map(|i| Measure::empty(self.measure_len(i))).collect();
        let mut part = Part::new(name, clef, program.min(127), measures);
        part.percussion = percussion;
        self.parts.push(part);
        self.parts.len() - 1
    }

    pub fn remove_part(&mut self, part: usize) -> Result<(), EditError> {
        if part >= self.parts.len() || self.parts.len() == 1 {
            return Err(EditError::OutOfRange);
        }
        self.parts.remove(part);
        Ok(())
    }

    pub fn update_part(&mut self, part: usize, u: PartUpdate) -> Result<(), EditError> {
        let p = self.parts.get_mut(part).ok_or(EditError::OutOfRange)?;
        if let Some(n) = u.name {
            p.name = n;
        }
        if let Some(c) = u.clef {
            p.clef = c;
        }
        if let Some(v) = u.program {
            p.program = v.min(127);
        }
        if let Some(v) = u.volume {
            p.volume = v.min(127);
        }
        if let Some(v) = u.muted {
            p.muted = v;
        }
        if let Some(v) = u.percussion {
            p.percussion = v;
        }
        if let Some(v) = u.solo {
            p.solo = v;
        }
        Ok(())
    }

    pub fn add_measures(&mut self, count: usize) {
        let starts: Vec<usize> = self.parts.iter().map(|p| p.measures.len()).collect();
        let lens: Vec<Vec<u32>> = starts.iter().map(|&s| (s..s + count).map(|i| self.measure_len(i)).collect()).collect();
        for (p, ls) in self.parts.iter_mut().zip(lens) {
            for l in ls {
                p.measures.push(Measure::empty(l));
            }
        }
    }

    /// Time signature in force at measure `m`.
    pub fn time_at(&self, m: usize) -> TimeSig {
        self.time_changes.iter().rev().find(|c| c.measure <= m).map_or(self.time, |c| c.time)
    }

    /// Length in ticks of measure `m`.
    pub fn measure_len(&self, m: usize) -> u32 {
        self.time_at(m).measure_ticks()
    }

    /// Absolute start tick of each of the first `n` measures.
    pub fn measure_starts(&self, n: usize) -> Vec<u32> {
        let mut out = Vec::with_capacity(n);
        let mut pos = 0;
        for m in 0..n {
            out.push(pos);
            pos += self.measure_len(m);
        }
        out
    }

    /// Measure index and offset inside it of an absolute tick.
    pub fn locate(&self, abs: u32) -> (usize, u32) {
        let (mut m, mut start) = (0usize, 0u32);
        loop {
            let len = self.measure_len(m);
            if abs < start + len {
                return (m, abs - start);
            }
            start += len;
            m += 1;
        }
    }

    /// Change the initial time signature (see `set_time_at`).
    pub fn set_time(&mut self, time: TimeSig) {
        self.set_time_at(0, time);
    }

    /// Set the time signature from measure `m` until the next change. Measures whose length
    /// changes are reset to rests; setting the signature already in force removes the change.
    pub fn set_time_at(&mut self, m: usize, time: TimeSig) {
        if m == 0 {
            self.time = time;
            self.time_changes.retain(|c| c.measure != 0);
        } else {
            self.time_changes.retain(|c| c.measure != m);
            self.time_changes.push(TimeChange { measure: m, time });
            self.time_changes.sort_by_key(|c| c.measure);
        }
        // Drop changes that repeat the signature already in force.
        let mut prev = self.time;
        self.time_changes.retain(|c| {
            let keep = c.time != prev;
            prev = c.time;
            keep
        });
        let end = self.time_changes.iter().find(|c| c.measure > m).map_or(usize::MAX, |c| c.measure);
        let len = time.measure_ticks();
        for p in &mut self.parts {
            let stop = end.min(p.measures.len());
            for i in m..stop {
                let me = &mut p.measures[i];
                let total: u32 = me.events.iter().map(|e| e.ticks).sum();
                let voices_ok = me.voices.iter().all(|v| v.is_empty() || v.iter().map(|e| e.ticks).sum::<u32>() == len);
                if total != len || !voices_ok {
                    *me = Measure::empty(len);
                }
            }
        }
    }
}

/// Document = score + linear undo/redo history (snapshot based).
pub struct Document {
    pub score: Score,
    undo: Vec<Score>,
    redo: Vec<Score>,
}

impl Document {
    pub fn new(score: Score) -> Document {
        Document { score, undo: Vec::new(), redo: Vec::new() }
    }

    /// Run an edit as one undoable step. Failed edits leave the score untouched.
    pub fn edit<T>(&mut self, f: impl FnOnce(&mut Score) -> Result<T, EditError>) -> Result<T, EditError> {
        let before = self.score.clone();
        let r = f(&mut self.score)?;
        if self.score != before {
            self.undo.push(before);
            self.redo.clear();
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
        Ok(r)
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.score, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.score, next));
                true
            }
            None => false,
        }
    }

    pub fn replace(&mut self, score: Score) {
        self.score = score;
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c4() -> Content {
        Content::note(vec![Pitch { step: 0, alter: 0, octave: 4 }], false)
    }

    fn score() -> Score {
        Score::new("t", TimeSig { num: 4, den: 4 }, 0, 2)
    }

    fn total(m: &Measure) -> u32 {
        m.events.iter().map(|e| e.ticks).sum()
    }

    #[test]
    fn midi_roundtrip() {
        assert_eq!(Pitch { step: 0, alter: 0, octave: 4 }.midi(), 60);
        assert_eq!(Pitch::from_midi(61, true), Pitch { step: 0, alter: 1, octave: 4 });
        assert_eq!(Pitch::from_midi(61, false), Pitch { step: 1, alter: -1, octave: 4 });
    }

    #[test]
    fn empty_measure_is_one_whole_rest() {
        let s = score();
        assert_eq!(s.parts[0].measures[0].events.len(), 1);
        assert_eq!(s.parts[0].measures[0].events[0].ticks, 1920);
    }

    #[test]
    fn enter_note_keeps_measure_full() {
        let mut s = score();
        let end = s.put(0, 0, 0, 0, PPQ, c4()).unwrap();
        assert_eq!(end, PPQ);
        let m = &s.parts[0].measures[0];
        assert_eq!(total(m), 1920);
        assert_eq!(m.events[0].content, c4());
        // remaining 3 quarters = one dotted half rest
        assert_eq!(m.events.iter().skip(1).map(|e| e.ticks).collect::<Vec<_>>(), vec![1440]);
    }

    #[test]
    fn overwrite_middle_and_delete_merges_rests() {
        let mut s = score();
        s.put(0, 0, 0, 480, 480, c4()).unwrap();
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
        s.delete_event(0, 0, 0, 480).unwrap();
        let m = &s.parts[0].measures[0];
        assert_eq!(m.events.len(), 1);
        assert_eq!(m.events[0].ticks, 1920);
    }

    #[test]
    fn rejects_bad_duration_and_range() {
        let mut s = score();
        assert_eq!(s.put(0, 0, 0, 0, 100, c4()), Err(EditError::BadDuration));
        assert_eq!(s.put(0, 0, 5, 0, 480, c4()), Err(EditError::OutOfRange));
        assert_eq!(s.put(0, 0, 0, 1920, 480, c4()), Err(EditError::OutOfRange));
    }

    #[test]
    fn note_longer_than_measure_is_clamped() {
        let mut s = score();
        s.put(0, 0, 0, 1440, 960, c4()).unwrap();
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
    }

    #[test]
    fn undo_redo() {
        let mut d = Document::new(score());
        d.edit(|s| s.put(0, 0, 0, 0, 480, c4())).unwrap();
        let after = d.score.clone();
        assert!(d.undo());
        assert_eq!(d.score, score());
        assert!(d.redo());
        assert_eq!(d.score, after);
        assert!(!d.redo());
    }

    #[test]
    fn failed_edit_not_recorded() {
        let mut d = Document::new(score());
        assert!(d.edit(|s| s.put(0, 0, 0, 0, 7, c4())).is_err());
        assert!(!d.undo());
    }

    #[test]
    fn transpose_spelling() {
        let mut s = score();
        s.put(0, 0, 0, 0, 480, c4()).unwrap();
        s.transpose_event(0, 0, 0, 0, 1).unwrap();
        assert_eq!(s.parts[0].measures[0].events[0].content, Content::note(vec![Pitch { step: 0, alter: 1, octave: 4 }], false));
        s.transpose_event(0, 0, 0, 0, -2).unwrap();
        assert_eq!(s.parts[0].measures[0].events[0].content, Content::note(vec![Pitch { step: 6, alter: 0, octave: 3 }], false));
    }

    #[test]
    fn time_change_resets_incompatible_measures() {
        let mut s = score();
        s.set_time(TimeSig { num: 3, den: 4 });
        assert_eq!(total(&s.parts[0].measures[0]), 1440);
    }

    fn p(step: u8, octave: i8) -> Pitch {
        Pitch { step, alter: 0, octave }
    }

    #[test]
    fn long_note_splits_with_tie_across_barline() {
        let mut s = score();
        let next = s.put_note(0, 0, 0, 1440, 960, vec![p(0, 4)]).unwrap();
        assert_eq!(next, (1, 480));
        let m0 = &s.parts[0].measures[0];
        assert_eq!(total(m0), 1920);
        assert_eq!(m0.events.last().unwrap().content, Content::note(vec![p(0, 4)], true));
        assert_eq!(s.parts[0].measures[1].events[0].content, Content::note(vec![p(0, 4)], false));
    }

    #[test]
    fn note_ending_on_barline_advances_to_next_measure() {
        let mut s = score();
        assert_eq!(s.put_note(0, 0, 1, 0, 1920, vec![p(0, 4)]).unwrap(), (2, 0));
        assert_eq!(s.parts[0].measures.len(), 3);
    }

    #[test]
    fn chords() {
        let mut s = score();
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 4)]).unwrap();
        s.add_pitch(0, 0, 0, 0, p(2, 4)).unwrap();
        s.add_pitch(0, 0, 0, 0, p(4, 4)).unwrap();
        s.add_pitch(0, 0, 0, 0, p(2, 4)).unwrap(); // duplicate ignored
        match &s.parts[0].measures[0].events[0].content {
            Content::Note { pitches, .. } => assert_eq!(pitches.len(), 3),
            _ => panic!(),
        }
        s.remove_pitch(0, 0, 0, 0, 64).unwrap();
        s.transpose_event(0, 0, 0, 0, 12).unwrap();
        match &s.parts[0].measures[0].events[0].content {
            Content::Note { pitches, .. } => {
                assert_eq!(pitches.iter().map(|x| x.midi()).collect::<Vec<_>>(), vec![72, 79])
            }
            _ => panic!(),
        }
    }

    #[test]
    fn toggle_tie_flag() {
        let mut s = score();
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 4)]).unwrap();
        s.toggle_tie(0, 0, 0, 0).unwrap();
        assert!(matches!(s.parts[0].measures[0].events[0].content, Content::Note { tie: true, .. }));
        assert!(s.toggle_tie(0, 0, 0, 480).is_err()); // rest
    }

    #[test]
    fn parts_add_update_remove() {
        let mut s = score();
        let i = s.add_part("Violonchelo", Clef::Bass, 42, false);
        assert_eq!(i, 1);
        assert_eq!(s.parts[1].measures.len(), 2);
        s.update_part(1, PartUpdate { volume: Some(60), muted: Some(true), ..Default::default() }).unwrap();
        assert_eq!((s.parts[1].volume, s.parts[1].muted), (60, true));
        s.remove_part(0).unwrap();
        assert_eq!(s.parts[0].name, "Violonchelo");
        assert!(s.remove_part(0).is_err()); // keep at least one
    }

    #[test]
    fn midi_channels_skip_percussion_channel() {
        let mut s = score();
        for _ in 0..10 {
            s.add_part("x", Clef::Treble, 0, false);
        }
        s.add_part("Bateria", Clef::Treble, 0, true);
        let ch = s.midi_channels();
        assert_eq!(&ch[..9], &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(ch[9], 10); // the 10th melodic part skips channel index 9
        assert_eq!(*ch.last().unwrap(), 9);
    }

    #[test]
    fn old_files_without_mixer_fields_load() {
        let j = r#"{"name":"P","clef":"treble","program":0,"measures":[]}"#;
        let p: Part = serde_json::from_str(j).unwrap();
        assert_eq!((p.volume, p.muted, p.solo), (100, false, false));
    }

    #[test]
    fn extra_voices_are_created_and_removed() {
        let mut s = score();
        assert_eq!(s.parts[0].measures[0].voice_count(), 1);
        s.put_note(0, 1, 0, 480, 480, vec![p(0, 4)]).unwrap();
        let m = &s.parts[0].measures[0];
        assert_eq!(m.voice_count(), 2);
        assert_eq!(m.voice(1).unwrap().iter().map(|e| e.ticks).sum::<u32>(), 1920);
        // voice 1 is untouched
        assert_eq!(m.events.len(), 1);
        assert!(s.delete_event(0, 2, 0, 0).is_err()); // voice 3 does not exist
        s.delete_event(0, 1, 0, 480).unwrap();
        assert_eq!(s.parts[0].measures[0].voice_count(), 1); // all rests -> voice removed
        assert!(s.put(0, 4, 0, 0, 480, c4()).is_err());
    }

    #[test]
    fn old_measures_without_voices_load() {
        let m: Measure = serde_json::from_str(r#"{"events":[]}"#).unwrap();
        assert!(m.voices.is_empty());
    }

    fn triplet_score() -> Score {
        let mut s = score();
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 4)]).unwrap();
        s.make_tuplet(0, 0, 0, 0, 3, 2).unwrap();
        s
    }

    #[test]
    fn make_triplet_splits_event_in_three_slots() {
        let s = triplet_score();
        let ev = &s.parts[0].measures[0].events;
        let g: Vec<_> = ev.iter().filter(|e| e.tuplet.is_some()).collect();
        assert_eq!(g.len(), 3);
        assert!(g.iter().all(|e| e.ticks == 160 && e.notated_ticks() == 240));
        assert_eq!(g[0].content, Content::note(vec![p(0, 4)], false)); // first slot keeps the note
        assert_eq!(g[1].content, Content::Rest);
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
        // unsupported ratio / already a tuplet
        let mut s2 = triplet_score();
        assert!(s2.make_tuplet(0, 0, 0, 0, 7, 4).is_err());
        assert!(s2.make_tuplet(0, 0, 0, 0, 3, 2).is_err());
    }

    #[test]
    fn enter_notes_inside_triplet() {
        let mut s = triplet_score();
        // written eighth at slot 2 -> sounds 160 ticks
        let next = s.put_note(0, 0, 0, 160, 240, vec![p(2, 4)]).unwrap();
        assert_eq!(next, (0, 320));
        let ev = &s.parts[0].measures[0].events;
        assert_eq!(ev[1].ticks, 160);
        assert_eq!(ev[1].content, Content::note(vec![p(2, 4)], false));
        // a written quarter (320 ticks) covers the last slot only if it fits -> error
        assert_eq!(s.put_note(0, 0, 0, 320, 480, vec![p(4, 4)]), Err(EditError::BadDuration));
        // written quarter at slot 1 replaces slots 1-2 by one 320-tick event
        s.put_note(0, 0, 0, 0, 480, vec![p(5, 4)]).unwrap();
        let ev = &s.parts[0].measures[0].events;
        assert_eq!((ev[0].ticks, ev[0].notated_ticks()), (320, 480));
        assert_eq!(ev[1].start, 320);
        assert!(ev[..2].iter().all(|e| e.tuplet.is_some()));
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
    }

    #[test]
    fn splitting_a_triplet_slot_keeps_the_group_full() {
        let mut s = triplet_score();
        // written 16th (real 80) at the first slot: remainder of the slot becomes a tuplet rest
        s.put_note(0, 0, 0, 0, 120, vec![p(0, 5)]).unwrap();
        let ev = &s.parts[0].measures[0].events;
        assert_eq!((ev[0].ticks, ev[1].start, ev[1].ticks), (80, 80, 80));
        assert!(ev[1].tuplet.is_some() && ev[1].content == Content::Rest);
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
    }

    #[test]
    fn delete_in_triplet_keeps_slot_and_overwrite_removes_group() {
        let mut s = triplet_score();
        s.delete_event(0, 0, 0, 0).unwrap();
        let ev = &s.parts[0].measures[0].events;
        assert_eq!(ev.iter().filter(|e| e.tuplet.is_some()).count(), 3);
        assert!(ev[0].content == Content::Rest);
        // a plain half note over the second slot removes the whole group
        s.put(0, 0, 0, 160, 960, c4()).unwrap();
        assert!(s.parts[0].measures[0].events.iter().all(|e| e.tuplet.is_none()));
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
    }

    #[test]
    fn clear_tuplet_restores_plain_event() {
        let mut s = triplet_score();
        s.clear_tuplet(0, 0, 0, 160).unwrap(); // any member works
        let ev = &s.parts[0].measures[0].events;
        assert_eq!((ev[0].start, ev[0].ticks, ev[0].tuplet), (0, 480, None));
        assert_eq!(ev[0].content, Content::note(vec![p(0, 4)], false));
        assert!(s.clear_tuplet(0, 0, 0, 0).is_err());
    }

    #[test]
    fn quintuplet_and_json_roundtrip() {
        let mut s = score();
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 4)]).unwrap();
        s.make_tuplet(0, 0, 0, 0, 5, 4).unwrap();
        let ev = &s.parts[0].measures[0].events;
        assert!(ev[..5].iter().all(|e| e.ticks == 96 && e.notated_ticks() == 120));
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Score>(&j).unwrap(), s);
        // plain events do not serialize a tuplet field (files stay compatible)
        assert!(!serde_json::to_string(&score()).unwrap().contains("tuplet"));
    }

    #[test]
    fn time_signature_changes_lengths_and_resets_only_affected_measures() {
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 6);
        s.put_note(0, 0, 1, 0, 480, vec![p(0, 4)]).unwrap(); // measure 1 keeps its content
        s.put_note(0, 0, 3, 0, 480, vec![p(2, 4)]).unwrap();
        s.set_time_at(2, TimeSig { num: 3, den: 4 });
        assert_eq!((s.measure_len(1), s.measure_len(2), s.measure_len(5)), (1920, 1440, 1440));
        assert_eq!(total(&s.parts[0].measures[1]), 1920);
        assert!(matches!(s.parts[0].measures[1].events[0].content, Content::Note { .. })); // untouched
        for i in 2..6 {
            assert_eq!(total(&s.parts[0].measures[i]), 1440, "measure {i}");
        }
        assert_eq!(s.measure_starts(4), vec![0, 1920, 3840, 5280]);
        assert_eq!(s.locate(5280), (3, 0));
        assert_eq!(s.locate(3839), (1, 1919));
        // appended measures use the signature in force
        s.add_measures(2);
        assert_eq!(total(&s.parts[0].measures[7]), 1440);
        // a note crossing into a measure of another length is split with a tie
        let next = s.put_note(0, 0, 1, 1440, 960, vec![p(4, 4)]).unwrap();
        assert_eq!(next, (2, 480));
        assert_eq!(s.parts[0].measures[2].events[0].ticks, 480);
        // back to the signature before: the change disappears
        s.set_time_at(2, TimeSig { num: 4, den: 4 });
        assert!(s.time_changes.is_empty());
        assert_eq!(s.measure_len(2), 1920);
        // a later change that repeats the previous one is dropped too
        s.set_time_at(3, TimeSig { num: 6, den: 8 });
        s.set_time_at(5, TimeSig { num: 6, den: 8 });
        assert_eq!(s.time_changes.len(), 1);
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Score>(&j).unwrap(), s);
    }

    #[test]
    fn put_abs_follows_time_changes() {
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 1);
        s.set_time_at(1, TimeSig { num: 3, den: 4 });
        // abs 1920 is the start of measure 1 (3/4); abs 3360 starts measure 2
        s.put_abs(0, 1920 + 960, 960, &[p(0, 4)]).unwrap(); // half note from beat 3 of measure 1, crosses the barline
        assert_eq!(s.parts[0].measures.len(), 3);
        assert!(matches!(s.parts[0].measures[1].events.last().unwrap().content, Content::Note { tie: true, .. }));
        assert!(matches!(s.parts[0].measures[2].events[0].content, Content::Note { tie: false, .. }));
    }

    fn pos(measure: usize, tick: u32) -> SpPos {
        SpPos { measure, tick }
    }

    #[test]
    fn spanners_are_validated_toggled_and_removed() {
        let mut s = score();
        let slur = Spanner { kind: SpannerKind::Slur, voice: 0, start: pos(0, 0), end: pos(0, 480), endings: 0 };
        assert!(s.toggle_spanner(0, slur).unwrap()); // added
        assert_eq!(s.parts[0].spanners, vec![slur]);
        assert!(!s.toggle_spanner(0, slur).unwrap()); // same one again removes it
        assert!(s.parts[0].spanners.is_empty());
        // invalid: end before start, past the last measure, past the barline, bad voice or part
        let bad = |s: &mut Score, sp| s.add_spanner(0, sp).is_err();
        assert!(bad(&mut s, Spanner { end: pos(0, 0), ..slur }));
        assert!(bad(&mut s, Spanner { end: pos(5, 0), ..slur }));
        assert!(bad(&mut s, Spanner { end: pos(1, 5000), ..slur }));
        assert!(bad(&mut s, Spanner { voice: 7, ..slur }));
        assert!(s.add_spanner(3, slur).is_err());
        // hairpin may end on the closing barline
        let hp = Spanner { kind: SpannerKind::Crescendo, voice: 0, start: pos(0, 0), end: pos(0, 1920), endings: 0 };
        s.add_spanner(0, hp).unwrap();
        s.add_spanner(0, Spanner { start: pos(1, 0), end: pos(1, 960), ..slur }).unwrap();
        // removing a range only touches overlapping spanners of that voice
        assert_eq!(s.remove_spanners_in(0, 1, pos(0, 0), pos(1, 0)).unwrap(), 0);
        assert_eq!(s.remove_spanners_in(0, 0, pos(0, 0), pos(0, 100)).unwrap(), 1);
        assert_eq!(s.parts[0].spanners.len(), 1);
        // they are saved with the score and stay when notes change
        s.put_note(0, 0, 1, 0, 480, vec![p(0, 4)]).unwrap();
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Score>(&j).unwrap().parts[0].spanners, s.parts[0].spanners);
        assert!(!serde_json::to_string(&score()).unwrap().contains("spanners"));
    }

    #[test]
    fn dynamics_and_articulations_edit_and_clear() {
        let mut s = score();
        for (i, st) in [0u32, 480, 960, 1440].into_iter().enumerate() {
            s.put_note(0, 0, 0, st, 480, vec![p(i as u8, 5)]).unwrap();
        }
        s.set_dynamic(0, 0, 0, 0, Some(Dynamic::Mf)).unwrap();
        assert_eq!(s.parts[0].measures[0].events[0].dynamic, Some(Dynamic::Mf));
        s.set_dynamic(0, 0, 0, 0, Some(Dynamic::Mf)).unwrap(); // same again removes it
        assert_eq!(s.parts[0].measures[0].events[0].dynamic, None);
        s.set_dynamic(0, 0, 0, 480, Some(Dynamic::F)).unwrap();
        s.set_dynamic(0, 0, 0, 480, Some(Dynamic::P)).unwrap(); // replaces
        assert_eq!(s.parts[0].measures[0].events[1].dynamic, Some(Dynamic::P));
        assert!(s.set_dynamic(0, 0, 0, 100, Some(Dynamic::F)).is_err());

        // articulation over a range: added to all, removed from all on the second call
        let (a, b) = (pos(0, 0), pos(0, 960));
        assert!(s.toggle_articulation(0, 0, a, b, Artic::Staccato).unwrap());
        let ev = &s.parts[0].measures[0].events;
        assert!(ev[..3].iter().all(|e| e.articulations == vec![Artic::Staccato]) && ev[3].articulations.is_empty());
        s.toggle_articulation(0, 0, pos(0, 480), pos(0, 480), Artic::Accent).unwrap();
        assert!(!s.toggle_articulation(0, 0, a, b, Artic::Staccato).unwrap());
        assert_eq!(s.parts[0].measures[0].events[1].articulations, vec![Artic::Accent]);
        assert!(s.toggle_articulation(0, 0, pos(0, 1900), pos(0, 1910), Artic::Tenuto).is_err()); // no notes there
        // deleting a note drops its articulations but keeps the dynamic
        s.delete_event(0, 0, 0, 480).unwrap();
        let e = &s.parts[0].measures[0].events[1];
        assert!(e.articulations.is_empty() && e.dynamic == Some(Dynamic::P));
        // saved and loaded
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Score>(&j).unwrap(), s);
        assert!(!serde_json::to_string(&score()).unwrap().contains("dynamic"));
    }

    #[test]
    fn repeats_and_voltas_unfold_into_playback_order() {
        let mut s = Score::new("r", TimeSig { num: 4, den: 4 }, 0, 6);
        assert_eq!(s.playback_order(), vec![0, 1, 2, 3, 4, 5]); // nothing to unfold
        // |: 0 1 :| 2 3   -> 0 1 0 1 2 3 4 5
        s.toggle_repeat(0, true).unwrap();
        s.toggle_repeat(1, false).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 0, 1, 2, 3, 4, 5]);
        // voltas on measures 2 (first ending, closed by a repeat) and 3 (second ending)
        let mut s = Score::new("v", TimeSig { num: 4, den: 4 }, 0, 5);
        s.toggle_repeat(0, true).unwrap();
        s.toggle_repeat(2, false).unwrap();
        let v = |m: usize, mask: u8| Spanner { kind: SpannerKind::Volta, voice: 0, start: pos(m, 0), end: pos(m, 1920), endings: mask };
        s.add_spanner(0, v(2, 0b01)).unwrap();
        s.add_spanner(0, v(3, 0b10)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 2, 0, 1, 3, 4]);
        // repeat without a start sign goes back to the beginning, and three plays
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 3);
        s.toggle_repeat(1, false).unwrap();
        s.parts[0].measures[1].end_repeat = 3;
        assert_eq!(s.playback_order(), vec![0, 1, 0, 1, 0, 1, 2]);
        assert!(s.toggle_repeat(9, true).is_err());
    }

    #[test]
    fn json_roundtrip() {
        let mut s = score();
        s.put(0, 0, 0, 0, 480, c4()).unwrap();
        let j = serde_json::to_string(&s).unwrap();
        let back: Score = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }
}

