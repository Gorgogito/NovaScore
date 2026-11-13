//! NovaScore core: score model, editing operations and undo/redo.
//! Licensed under GPL-3.0-or-later.

pub mod beats;
pub mod assist;
pub mod chords;
pub mod io;
pub mod mscx;
pub mod mscx_write;
pub mod quantize;
pub mod tab;

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

/// Interval between written and sounding pitch of a transposing instrument, with the MuseScore sign:
/// sounding = written + interval (a B♭ clarinet is `diatonic -1, chromatic -2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transpose {
    pub diatonic: i8,
    pub chromatic: i8,
}

impl Transpose {
    /// Move a pitch by `sign` times the interval, keeping the spelling consistent with the diatonic step.
    fn shift(self, p: Pitch, sign: i32) -> Pitch {
        const SEMI: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
        let total = p.octave as i32 * 7 + p.step as i32 + sign * self.diatonic as i32;
        let (octave, step) = (total.div_euclid(7), total.rem_euclid(7));
        let target = p.midi() + sign * self.chromatic as i32;
        let alter = target - ((octave + 1) * 12 + SEMI[step as usize]);
        if alter.abs() > 2 {
            return Pitch::from_midi(target, alter < 0);
        }
        Pitch { step: step as u8, alter: alter as i8, octave: octave as i8 }
    }

    /// Pitch as the player reads it, from the sounding one.
    pub fn to_written(self, p: Pitch) -> Pitch {
        self.shift(p, -1)
    }

    /// Sounding pitch of a written one.
    pub fn to_sounding(self, p: Pitch) -> Pitch {
        self.shift(p, 1)
    }

    /// Key signature (number of sharps, flats negative) the player reads for a sounding major key.
    pub fn written_fifths(self, fifths: i8) -> i8 {
        const BASE: [i32; 7] = [0, 2, 4, -1, 1, 3, 5];
        let f = fifths as i32;
        let tonic = Pitch { step: (f * 4).rem_euclid(7) as u8, alter: ((f + 1).div_euclid(7)) as i8, octave: 4 };
        let w = self.to_written(tonic);
        let mut out = BASE[w.step as usize] + 7 * w.alter as i32;
        while out > 7 {
            out -= 12;
        }
        while out < -7 {
            out += 12;
        }
        out as i8
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
    /// Lyric syllables sung on this event (one per verse).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lyrics: Vec<Lyric>,
    /// Free text above the staff that starts at this event (staff text: "pizz.", "dolce"...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tablature: string chosen for each pitch (ascending), numbered as in MusicXML (1 = highest string); 0 = automatic.
    /// Empty when the player did not choose; editing the pitches of the event drops it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strings: Vec<u8>,
    /// Guitar technique that starts at this note (hammer-on / pull-off or slide towards the next note, bend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tech: Option<Tech>,
}

/// Guitar technique written on a tablature note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tech {
    /// Slurred link to the next note: hammer-on when it is higher, pull-off when lower.
    Hammer,
    /// Slide to the next note.
    Slide,
    /// Bend up by this many semitones (1..=4).
    Bend(u8),
}

/// How a lyric syllable joins the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Syllabic {
    /// A whole word.
    #[default]
    Single,
    /// First syllable of a word (a hyphen follows).
    Begin,
    /// Inner syllable (hyphens on both sides).
    Middle,
    /// Last syllable of a word.
    End,
}

/// A lyric syllable. `verse` is 0-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyric {
    #[serde(default)]
    pub verse: u8,
    pub text: String,
    #[serde(default)]
    pub syllabic: Syllabic,
    /// Melisma: the extender line runs from the syllable to this position (the end of the last
    /// note it covers), anchored to time like a spanner. Notes starting before it are covered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extend_to: Option<SpPos>,
}

/// Notes covered by melisma lines, keyed by `(measure, start tick)`: `(verse, is the last covered note)`.
#[derive(Debug, Default)]
pub struct LyricCovers {
    pub covered: std::collections::HashMap<(usize, u32), Vec<(u8, bool)>>,
    /// Syllables `(measure, start tick, verse)` whose line covers at least one note.
    pub origins: std::collections::HashSet<(usize, u32, u8)>,
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
    /// Mordent: main note, the note below, main note.
    Mordent,
    /// Upper mordent (pralltriller): main note, the note above, main note.
    ShortTrill,
    /// Turn: the note above, main, the note below, main.
    Turn,
    /// Inverted turn: the note below, main, the note above, main.
    InvertedTurn,
}

/// Every articulation and ornament, in the order the toolbar shows them.
pub const ALL_ARTICS: [Artic; 10] = [
    Artic::Staccato,
    Artic::Staccatissimo,
    Artic::Tenuto,
    Artic::Accent,
    Artic::Marcato,
    Artic::Portato,
    Artic::Mordent,
    Artic::ShortTrill,
    Artic::Turn,
    Artic::InvertedTurn,
];

impl Artic {
    pub fn name(self) -> &'static str {
        match self {
            Artic::Staccato => "staccato",
            Artic::Staccatissimo => "staccatissimo",
            Artic::Tenuto => "tenuto",
            Artic::Accent => "accent",
            Artic::Marcato => "marcato",
            Artic::Portato => "portato",
            Artic::Mordent => "mordent",
            Artic::ShortTrill => "shorttrill",
            Artic::Turn => "turn",
            Artic::InvertedTurn => "invertedturn",
        }
    }

    pub fn from_name(s: &str) -> Option<Artic> {
        ALL_ARTICS.into_iter().find(|a| a.name() == s.trim())
    }

    /// Ornaments change the notes that sound; the other signs only shape one note.
    pub fn is_ornament(self) -> bool {
        matches!(self, Artic::Mordent | Artic::ShortTrill | Artic::Turn | Artic::InvertedTurn)
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

/// Supported ratios. Those that divide 480 ticks per quarter exactly (3:2, 5:4...) have equal slots;
/// the others (7:4, 7:8) round each position to a whole tick, see [`Tuplet::tick_at`].
pub fn tuplet_ratio_supported(actual: u8, normal: u8) -> bool {
    matches!((actual, normal), (2, 3) | (3, 2) | (4, 3) | (5, 4) | (6, 4) | (7, 4) | (7, 8))
}

impl Tuplet {
    /// Tick of the point `off` written ticks after the start of the group. Positions are rounded
    /// to a whole tick from the group start, so slots of a 7:4 differ by one tick but add up exactly.
    pub fn tick_at(&self, off: u32) -> u32 {
        self.start + (off * self.normal as u32 + self.actual as u32 / 2) / self.actual as u32
    }

    /// Written ticks since the start of the group at a tick of the group (inverse of `tick_at`).
    /// Written positions are multiples of the smallest note value, which absorbs the rounding.
    pub fn off_at(&self, tick: u32) -> u32 {
        let off = (tick.saturating_sub(self.start) * self.actual as u32 + self.normal as u32 / 2) / self.normal as u32;
        (off + MIN_TICKS / 2) / MIN_TICKS * MIN_TICKS
    }
}

/// Written value of a tuplet event that sounds `ticks` (the standard value that sounds within a tick of it,
/// which is the exact one when the ratio divides evenly).
pub fn tuplet_notated(ticks: u32, actual: u8, normal: u8) -> Option<u32> {
    let (a, n) = (actual as u32, normal as u32);
    if (ticks * a) % n == 0 {
        return Some(ticks * a / n);
    }
    let mut v = PPQ * 4;
    while v >= MIN_TICKS {
        for c in [v, v + v / 2] {
            if valid_duration(c) && (c * n).abs_diff(ticks * a) <= a {
                return Some(c);
            }
        }
        v /= 2;
    }
    None
}

impl Event {
    pub fn new(start: u32, ticks: u32, content: Content) -> Event {
        Event { start, ticks, content, tuplet: None, extras: Vec::new(), dynamic: None, articulations: Vec::new(), lyrics: Vec::new(), text: None, strings: Vec::new(), tech: None }
    }

    /// Written (notated) duration: the sounding duration scaled back by the tuplet ratio.
    pub fn notated_ticks(&self) -> u32 {
        match self.tuplet {
            Some(t) => tuplet_notated(self.ticks, t.actual, t.normal).unwrap_or(self.ticks * t.actual as u32 / t.normal as u32),
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
    /// Navigation markers of the measure (segno, coda, "To Coda", fine). Applies to every part.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Jump taken at the end of the measure (D.C., D.S. and their "al Fine"/"al Coda" forms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<Jump>,
}

/// Navigation markers. `Segno` and `Coda` are jump targets; `ToCoda` and `Fine` act at the end of their measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Marker {
    Segno,
    Coda,
    ToCoda,
    Fine,
}

/// Jumps. `Dc` returns to the beginning, `Ds` to the segno; "al Fine" stops at the fine marker and
/// "al Coda" leaves for the coda at the "To Coda" marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Jump {
    Dc,
    DcAlFine,
    DcAlCoda,
    Ds,
    DsAlFine,
    DsAlCoda,
}

impl Jump {
    pub fn to_segno(self) -> bool {
        matches!(self, Jump::Ds | Jump::DsAlFine | Jump::DsAlCoda)
    }
    pub fn al_fine(self) -> bool {
        matches!(self, Jump::DcAlFine | Jump::DsAlFine)
    }
    pub fn al_coda(self) -> bool {
        matches!(self, Jump::DcAlCoda | Jump::DsAlCoda)
    }
    pub fn text(self) -> &'static str {
        match self {
            Jump::Dc => "D.C.",
            Jump::DcAlFine => "D.C. al Fine",
            Jump::DcAlCoda => "D.C. al Coda",
            Jump::Ds => "D.S.",
            Jump::DsAlFine => "D.S. al Fine",
            Jump::DsAlCoda => "D.S. al Coda",
        }
    }
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
    /// Transposing instrument: notes are stored at sounding pitch and shown at written pitch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transpose: Option<Transpose>,
    /// Tablature: MIDI pitch of each open string (sounding, lowest string first). When set a tab staff is drawn
    /// under the staff, and the frets go to MusicXML and MuseScore files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Vec<u8>>,
    /// With a tuning: show only the tablature, without the standard staff.
    #[serde(default, skip_serializing_if = "is_false")]
    pub tab_only: bool,
    /// Only while writing a MuseScore file: this part is the tablature twin of the one before it.
    #[serde(skip)]
    pub tab_staff: bool,
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
    #[serde(default)]
    pub transpose: Option<Transpose>,
    #[serde(default)]
    pub tuning: Option<Vec<u8>>,
}

fn default_volume() -> u8 {
    100
}

impl Part {
    fn measure_events_mut(&mut self, m: usize, voice: usize) -> &mut Vec<Event> {
        self.measures[m].voice_mut(voice).expect("voice checked by caller")
    }

    pub fn new(name: impl Into<String>, clef: Clef, program: u8, measures: Vec<Measure>) -> Part {
        Part { name: name.into(), clef, program, volume: default_volume(), muted: false, solo: false, percussion: false, ms: None, transpose: None, tuning: None, tab_only: false, tab_staff: false, spanners: Vec::new(), measures }
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
    /// `Some(None)` clears the transposition; absent leaves it unchanged.
    #[serde(default, deserialize_with = "double_option")]
    pub transpose: Option<Option<Transpose>>,
    /// Tablature: `Some(Some(strings))` shows a tab staff with that tuning, `Some(None)` hides it.
    #[serde(default, deserialize_with = "double_option")]
    pub tuning: Option<Option<Vec<u8>>>,
    /// Show only the tablature (needs a tuning).
    pub tab_only: Option<bool>,
}

fn double_option<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Ok(Some(Option::<T>::deserialize(d)?))
}

/// A time signature that starts at `measure` (0-based) and lasts until the next change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeChange {
    pub measure: usize,
    pub time: TimeSig,
}

/// A key signature (sharps positive, flats negative) that starts at `measure` (0-based) and lasts until the next change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyChange {
    pub measure: usize,
    pub fifths: i8,
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
    /// Initial key signature as number of sharps (positive) or flats (negative).
    pub fifths: i8,
    /// Later key changes (concert key), sorted by measure and never equal to the key in force before them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_changes: Vec<KeyChange>,
    pub tempo: u32,
    pub parts: Vec<Part>,
    /// Style settings (`score_style.mss`) of the MuseScore file this score came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_style: Option<String>,
    /// Chord symbols shown above the first staff (each lasts until the next one).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chords: Vec<chords::ChordMark>,
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
    let (o0, o1) = (t.off_at(from), t.off_at(to));
    let mut out = Vec::new();
    let mut off = o0;
    for piece in split_standard(o1 - o0) {
        let (a, b) = (t.tick_at(off), t.tick_at(off + piece));
        out.push(Event { start: a, ticks: b - a, content: Content::Rest, tuplet: Some(t), extras: Vec::new(), dynamic: None, articulations: Vec::new(), lyrics: Vec::new(), text: None, strings: Vec::new(), tech: None });
        off += piece;
    }
    Ok(out)
}

impl Measure {
    pub fn empty(len: u32) -> Measure {
        Measure { events: fill_rests(Vec::new(), len), voices: Vec::new(), extras: Vec::new(), start_repeat: false, end_repeat: 0, markers: Vec::new(), jump: None }
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
            key_changes: Vec::new(),
            tempo: 100,
            ms_style: None,
            chords: Vec::new(),
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
        if !valid_duration(notated) || start < t.start {
            return Err(EditError::BadDuration);
        }
        let real = t.tick_at(t.off_at(start) + notated) - start;
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
        next.push(Event { start, ticks: real, content, tuplet: Some(t), extras: Vec::new(), dynamic: None, articulations: Vec::new(), lyrics: Vec::new(), text: None, strings: Vec::new(), tech: None });
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
        if span % normal as u32 != 0 || !valid_duration(span / normal as u32) {
            return Err(EditError::BadDuration);
        }
        let each = span / normal as u32; // written value of one slot
        let t = Tuplet { actual, normal, start };
        let slots: Vec<Event> = (0..actual as u32)
            .map(|i| Event { start: t.tick_at(i * each), ticks: t.tick_at((i + 1) * each) - t.tick_at(i * each), content: if i == 0 { e.content.clone() } else { Content::Rest }, tuplet: Some(t), extras: if i == 0 { e.extras.clone() } else { Vec::new() }, dynamic: if i == 0 { e.dynamic } else { None }, articulations: if i == 0 { e.articulations.clone() } else { Vec::new() }, lyrics: if i == 0 { e.lyrics.clone() } else { Vec::new() }, text: if i == 0 { e.text.clone() } else { None }, strings: Vec::new(), tech: None })
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
        events[idx].lyrics.clear();
        events[idx].strings.clear();
        // Merge adjacent plain rests back into standard values (tuplet events keep their slots).
        let mut merged: Vec<Event> = Vec::new();
        // A run of merged rests keeps the dynamic and markup of its first event.
        let mut run: Option<(u32, u32, Option<Dynamic>, Vec<Extra>, Option<String>)> = None;
        let flush = |merged: &mut Vec<Event>, (s, t, dynamic, extras, text): (u32, u32, Option<Dynamic>, Vec<Extra>, Option<String>)| {
            let mut p = s;
            for (i, v) in split_standard(t).into_iter().enumerate() {
                let mut ev = Event::new(p, v, Content::Rest);
                if i == 0 {
                    ev.dynamic = dynamic;
                    ev.extras = extras.clone();
                    ev.text = text.clone();
                }
                merged.push(ev);
                p += v;
            }
        };
        for e in events.drain(..) {
            if e.content == Content::Rest && e.tuplet.is_none() {
                run = Some(match run {
                    Some((s, t, d, x, tx)) => (s, t + e.ticks, d.or(e.dynamic), x, tx.or(e.text.clone())),
                    None => (e.start, e.ticks, e.dynamic, e.extras.clone(), e.text.clone()),
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
                e.strings.clear();
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
                    e.strings.clear();
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

    /// Transposes every pitched part from measure `from` to `to` (inclusive) by `semitones`, together with the key
    /// signature (the nearest one to the new tonic, so the notes keep their place in the scale) and the chord symbols.
    /// Notes are spelled with the key of their measure. Refuses a transposition that leaves the MIDI range; strings
    /// pinned on tablature and per-note extras of MuseScore are dropped, as when a single note is moved.
    pub fn transpose_range(&mut self, from: usize, to: usize, semitones: i32) -> Result<(), EditError> {
        let n = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        if semitones == 0 || from >= n || from > to {
            return Err(EditError::OutOfRange);
        }
        let to = to.min(n - 1);
        let mut keys: Vec<i8> = (0..n).map(|m| self.key_at(m)).collect();
        for k in keys.iter_mut().take(to + 1).skip(from) {
            let tonic = (*k as i32 * 7).rem_euclid(12);
            *k = assist::fifths_of_pc(tonic + semitones);
        }
        let in_range = |p: &Part| {
            !p.percussion
                && p.measures.iter().enumerate().skip(from).take(to + 1 - from).all(|(_, me)| {
                    me.events.iter().chain(me.voices.iter().flatten()).all(|e| match &e.content {
                        Content::Note { pitches, .. } => pitches.iter().all(|q| (0..=127).contains(&(q.midi() + semitones))),
                        _ => true,
                    })
                })
        };
        if self.parts.iter().any(|p| !p.percussion && !in_range(p)) {
            return Err(EditError::OutOfRange);
        }
        for p in self.parts.iter_mut().filter(|p| !p.percussion) {
            for m in from..=to.min(p.measures.len().saturating_sub(1)) {
                let sharps = keys[m] >= 0;
                let me = &mut p.measures[m];
                for e in me.events.iter_mut().chain(me.voices.iter_mut().flatten()) {
                    if let Content::Note { pitches, .. } = &mut e.content {
                        for q in pitches.iter_mut() {
                            *q = Pitch::from_midi(q.midi() + semitones, sharps);
                        }
                        e.extras.retain(|x| !matches!(x.place, ExtraPlace::Note(_)));
                        e.strings.clear();
                    }
                }
            }
        }
        for c in self.chords.iter_mut().filter(|c| c.measure >= from && c.measure <= to) {
            c.root = (c.root as i32 + semitones).rem_euclid(12) as u8;
            c.bass = (c.bass as i32 + semitones).rem_euclid(12) as u8;
        }
        self.fifths = keys[0];
        self.key_changes = (1..n).filter(|&m| keys[m] != keys[m - 1]).map(|m| KeyChange { measure: m, fifths: keys[m] }).collect();
        Ok(())
    }

    pub fn transpose_event(&mut self, part: usize, voice: usize, m: usize, start: u32, semitones: i32) -> Result<(), EditError> {
        let sharps = if semitones != 0 { semitones > 0 } else { self.key_at(m) >= 0 };
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        if let Content::Note { pitches, .. } = &mut e.content {
            for p in pitches.iter_mut() {
                *p = Pitch::from_midi(p.midi() + semitones, sharps);
            }
            e.extras.retain(|x| !matches!(x.place, ExtraPlace::Note(_)));
            e.strings.clear();
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
        self.put_abs_voice_tied(part, voice, abs, ticks, pitches, false)
    }

    /// Like [`Score::put_abs_voice`]; with `tie_out` the last piece is also tied to what follows.
    pub fn put_abs_voice_tied(&mut self, part: usize, voice: usize, abs: u32, ticks: u32, pitches: &[Pitch], tie_out: bool) -> Result<(), EditError> {
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
                self.put(part, voice, m, p, piece, Content::note(pitches.to_vec(), !last || tie_out))?;
                p += piece;
            }
            let used = p - start;
            pos += used;
            remaining -= used;
        }
        Ok(())
    }

    /// Build one beat (a quarter note, starting at absolute tick `abs`) as a group of three
    /// triplet eighths (3:2). See [`Score::put_tuplet_beat`].
    pub fn put_triplet_beat(&mut self, part: usize, voice: usize, abs: u32, items: &[(u32, u32, Vec<Pitch>, bool)]) -> Result<(), EditError> {
        self.put_tuplet_beat(part, voice, abs, 3, items)
    }

    /// Build one beat (a quarter note starting at absolute tick `abs`) as a tuplet group of
    /// `slots` equal slots: 3 (3:2, 160 ticks each), 5 (5:4, 96) or 6 (6:4, 80). `items` are
    /// `(first slot, slots, pitches, tied to next)`; the slots left empty become rests and
    /// whatever the voice had in the beat is replaced. A note or rest longer than the notation
    /// allows (e.g. 5 of 6 slots) is split into tied pieces. The beat must lie inside one
    /// measure. Used by importers.
    pub fn put_tuplet_beat(&mut self, part: usize, voice: usize, abs: u32, slots: u32, items: &[(u32, u32, Vec<Pitch>, bool)]) -> Result<(), EditError> {
        self.put_tuplet_group(part, voice, abs, 1, slots, items)
    }

    /// Like `put_tuplet_beat` for a group that spans `beats` quarter notes: 3, 5, 6 or 7 notes in one beat,
    /// or 7 notes in two beats (a 7:4 of eighths, the group of the sample `septillos`).
    pub fn put_tuplet_group(&mut self, part: usize, voice: usize, abs: u32, beats: u32, slots: u32, items: &[(u32, u32, Vec<Pitch>, bool)]) -> Result<(), EditError> {
        // (actual, normal) of the group and the longest span (in slots) with a standard value
        let (normal, max_n) = match (beats, slots) {
            (1, 3) => (2u8, 2u32),
            (1, 5) | (1, 6) | (1, 7) => (4, 4),
            (2, 7) => (4, 4),
            _ => return Err(EditError::BadDuration),
        };
        let span = PPQ * beats;
        // written value of one slot: an eighth for triplets and the two-beat septuplet, a sixteenth for the others
        let written = span / normal as u32;
        if part >= self.parts.len() || voice >= MAX_VOICES || items.is_empty() {
            return Err(EditError::OutOfRange);
        }
        let (m, start) = self.locate(abs);
        let len = self.measure_len(m);
        if start + span > len {
            return Err(EditError::OutOfRange);
        }
        let mut sorted: Vec<&(u32, u32, Vec<Pitch>, bool)> = items.iter().collect();
        sorted.sort_by_key(|i| i.0);
        let mut slot = 0u32;
        for i in &sorted {
            if i.1 == 0 || i.0 < slot || i.0 + i.1 > slots || i.2.is_empty() {
                return Err(EditError::BadDuration);
            }
            slot = i.0 + i.1;
        }
        while self.parts[part].measures.len() <= m {
            let l = self.measure_len(self.parts[part].measures.len());
            self.parts[part].measures.push(Measure::empty(l));
        }
        let t = Tuplet { actual: slots as u8, normal, start };
        let mk = |slot: u32, n: u32, content: Content| Event {
            start: t.tick_at(slot * written),
            ticks: t.tick_at((slot + n) * written) - t.tick_at(slot * written),
            content,
            tuplet: Some(t),
            extras: Vec::new(),
            dynamic: None,
            articulations: Vec::new(),
            lyrics: Vec::new(),
            text: None,
            strings: Vec::new(),
            tech: None,
        };
        let mut new_events: Vec<Event> = Vec::new();
        // pieces of at most `max_n` slots; a note keeps its tie on the last piece only
        let push_span = |new_events: &mut Vec<Event>, first: u32, n: u32, pitches: Option<(&Vec<Pitch>, bool)>| {
            let (mut at, mut left) = (first, n);
            while left > 0 {
                let piece = left.min(max_n);
                let more = left > piece;
                let content = match pitches {
                    Some((p, tie)) => Content::note(p.clone(), tie || more),
                    None => Content::Rest,
                };
                new_events.push(mk(at, piece, content));
                at += piece;
                left -= piece;
            }
        };
        let mut pos = 0u32;
        for i in &sorted {
            if i.0 > pos {
                push_span(&mut new_events, pos, i.0 - pos, None);
            }
            push_span(&mut new_events, i.0, i.1, Some((&i.2, i.3)));
            pos = i.0 + i.1;
        }
        if pos < slots {
            push_span(&mut new_events, pos, slots - pos, None);
        }
        let meas = self.measure_mut(part, m)?;
        let events = meas.voice_create(voice, len);
        let mut kept: Vec<Event> = events.iter().filter(|e| e.start + e.ticks <= start || e.start >= start + PPQ).cloned().collect();
        kept.extend(new_events);
        *events = fill_rests(kept, len);
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

    /// Set (or clear with empty text) the staff text that starts at an event, note or rest.
    pub fn set_text(&mut self, part: usize, voice: usize, m: usize, start: u32, text: &str) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        let t = text.trim();
        e.text = if t.is_empty() { None } else { Some(t.to_string()) };
        Ok(())
    }

    /// Append `other` after the last measure of this score (movements of one piece): parts are matched by
    /// position, a part missing on either side is filled with rests, and a different time signature
    /// becomes a change at the join; so does a different key signature. The tempo stays this score's. Returns true
    /// when the key of `other` differs from the one in force at the join (a key change was written there).
    pub fn append(&mut self, other: &Score) -> bool {
        let n0 = self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        // pad every part to the same length with the lengths in force
        let pad = |s: &mut Score, upto: usize| {
            let lens: Vec<u32> = (0..upto).map(|m| s.measure_len(m)).collect();
            for p in &mut s.parts {
                while p.measures.len() < upto {
                    p.measures.push(Measure::empty(lens[p.measures.len()]));
                }
            }
        };
        pad(self, n0);
        let join_time = self.time_at(n0);
        if other.time != join_time {
            self.time_changes.push(TimeChange { measure: n0, time: other.time });
        }
        for c in &other.time_changes {
            self.time_changes.push(TimeChange { measure: c.measure + n0, time: c.time });
        }
        let join_key = self.key_at(n0);
        let key_differs = other.fifths != join_key;
        if key_differs {
            self.key_changes.push(KeyChange { measure: n0, fifths: other.fifths });
        }
        for c in &other.key_changes {
            self.key_changes.push(KeyChange { measure: c.measure + n0, fifths: c.fifths });
        }
        self.key_changes.sort_by_key(|c| c.measure);
        let mut curk = self.fifths;
        self.key_changes.retain(|c| {
            let keep = c.fifths != curk;
            curk = c.fifths;
            keep
        });
        // drop changes equal to the signature already in force
        self.time_changes.sort_by_key(|c| c.measure);
        let mut cur = self.time;
        self.time_changes.retain(|c| {
            let keep = c.time != cur;
            cur = c.time;
            keep
        });
        let n1 = other.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        while self.parts.len() < other.parts.len() {
            let src = &other.parts[self.parts.len()];
            let mut p = Part::new(src.name.clone(), src.clef, src.program, Vec::new());
            p.percussion = src.percussion;
            p.transpose = src.transpose;
            p.tuning = src.tuning.clone();
            self.parts.push(p);
            pad(self, n0);
        }
        for (i, p) in self.parts.iter_mut().enumerate() {
            match other.parts.get(i) {
                Some(o) => {
                    p.measures.extend(o.measures.iter().cloned());
                    for sp in &o.spanners {
                        let mut sp = sp.clone();
                        sp.start.measure += n0;
                        sp.end.measure += n0;
                        p.spanners.push(sp);
                    }
                    // melisma lines anchored to a time
                    for m in p.measures[n0..].iter_mut() {
                        for e in m.events.iter_mut().chain(m.voices.iter_mut().flatten()) {
                            for l in &mut e.lyrics {
                                if let Some(end) = &mut l.extend_to {
                                    end.measure += n0;
                                }
                            }
                        }
                    }
                }
                None => {}
            }
        }
        pad(self, n0 + n1);
        for c in &other.chords {
            let mut c = *c;
            c.measure += n0;
            self.chords.push(c);
        }
        key_differs
    }

    /// Pin the note `midi` of a chord to a string (1 = highest string, as in MusicXML); 0 goes back to automatic.
    pub fn set_string(&mut self, part: usize, voice: usize, m: usize, start: u32, midi: i32, string: u8) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        let Content::Note { pitches, .. } = &e.content else { return Err(EditError::OutOfRange) };
        let i = pitches.iter().position(|p| p.midi() == midi).ok_or(EditError::OutOfRange)?;
        if e.strings.len() != pitches.len() {
            e.strings = vec![0; pitches.len()];
        }
        e.strings[i] = string;
        if e.strings.iter().all(|&x| x == 0) {
            e.strings.clear();
        }
        Ok(())
    }

    /// Set (or clear with `None`) a guitar technique on a note.
    pub fn set_tech(&mut self, part: usize, voice: usize, m: usize, start: u32, tech: Option<Tech>) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        if !matches!(e.content, Content::Note { .. }) {
            return Err(EditError::OutOfRange);
        }
        let tech = match tech {
            Some(Tech::Bend(n)) => Some(Tech::Bend(n.clamp(1, 4))),
            t => t,
        };
        e.tech = if e.tech == tech { None } else { tech };
        Ok(())
    }

    /// Set (or clear with `None`) the end of the melisma line of a verse's syllable.
    pub fn set_lyric_extend(&mut self, part: usize, voice: usize, m: usize, start: u32, verse: u8, end: Option<SpPos>) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        let l = e.lyrics.iter_mut().find(|l| l.verse == verse).ok_or(EditError::OutOfRange)?;
        l.extend_to = end;
        Ok(())
    }

    /// Roll melisma ends that overflow their measure (importers add durations to the start tick)
    /// into the following measures.
    pub fn normalize_lyric_extends(&mut self) {
        let lens: Vec<u32> = (0..self.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0)).map(|m| self.measure_len(m)).collect();
        for p in &mut self.parts {
            for m in &mut p.measures {
                for e in m.events.iter_mut().chain(m.voices.iter_mut().flatten()) {
                    for l in &mut e.lyrics {
                        if let Some(end) = &mut l.extend_to {
                            while lens.get(end.measure).map_or(false, |&len| len > 0 && end.tick >= len) {
                                end.tick -= lens[end.measure];
                                end.measure += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Notes of a voice that lie under a melisma line. The last covered note of each line is marked.
    pub fn lyric_covers(&self, part: usize, voice: usize) -> LyricCovers {
        let mut out = LyricCovers::default();
        let Some(p) = self.parts.get(part) else { return out };
        let all: Vec<(usize, &Event)> = p.measures.iter().enumerate().flat_map(|(mi, m)| m.voice(voice).into_iter().flatten().map(move |e| (mi, e))).collect();
        for (i, (mi, e)) in all.iter().enumerate() {
            for l in &e.lyrics {
                let Some(end) = l.extend_to else { continue };
                let mut last: Option<(usize, u32)> = None;
                for (m2, e2) in &all[i + 1..] {
                    if (SpPos { measure: *m2, tick: e2.start }) >= end {
                        break;
                    }
                    if matches!(e2.content, Content::Note { .. }) {
                        out.covered.entry((*m2, e2.start)).or_default().push((l.verse, false));
                        last = Some((*m2, e2.start));
                    }
                }
                if let Some(k) = last {
                    out.origins.insert((*mi, e.start, l.verse));
                    if let Some(v) = out.covered.get_mut(&k).and_then(|v| v.iter_mut().rev().find(|x| x.0 == l.verse)) {
                        v.1 = true;
                    }
                }
            }
        }
        out
    }

    /// Set the syllable of a verse on a note (an empty text removes it).
    pub fn set_lyric(&mut self, part: usize, voice: usize, m: usize, start: u32, verse: u8, text: &str, syllabic: Syllabic) -> Result<(), EditError> {
        let events = self.voice_events_mut(part, voice, m)?;
        let e = events.iter_mut().find(|e| e.start == start).ok_or(EditError::OutOfRange)?;
        if !matches!(e.content, Content::Note { .. }) {
            return Err(EditError::OutOfRange);
        }
        let extend_to = e.lyrics.iter().find(|l| l.verse == verse).and_then(|l| l.extend_to);
        e.lyrics.retain(|l| l.verse != verse);
        let text = text.trim();
        if !text.is_empty() {
            e.lyrics.push(Lyric { verse, text: text.to_string(), syllabic, extend_to });
            e.lyrics.sort_by_key(|l| l.verse);
        }
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

    /// Add or remove a navigation marker on a measure (applies to every part).
    pub fn toggle_marker(&mut self, measure: usize, marker: Marker) -> Result<(), EditError> {
        if self.parts.is_empty() || self.parts.iter().any(|p| measure >= p.measures.len()) {
            return Err(EditError::OutOfRange);
        }
        for p in &mut self.parts {
            let ms = &mut p.measures[measure].markers;
            if let Some(i) = ms.iter().position(|m| *m == marker) {
                ms.remove(i);
            } else {
                ms.push(marker);
            }
        }
        Ok(())
    }

    /// Set the jump of a measure; the same jump again (or `None`) removes it.
    pub fn set_jump(&mut self, measure: usize, jump: Option<Jump>) -> Result<(), EditError> {
        if self.parts.is_empty() || self.parts.iter().any(|p| measure >= p.measures.len()) {
            return Err(EditError::OutOfRange);
        }
        let cur = self.parts[0].measures[measure].jump;
        let new = if cur == jump { None } else { jump };
        for p in &mut self.parts {
            p.measures[measure].jump = new;
        }
        Ok(())
    }

    /// Measures in the order they are played: repeats are unfolded and each volta is only played
    /// on its pass. A measure index can appear several times.
    ///
    /// Jumps are taken once, at the end of their measure (after its repeat sign). From then on repeat
    /// signs are ignored and a volta is only played if it is the last ending. "al Fine" stops after the
    /// `Fine` measure; "al Coda" leaves at the `ToCoda` measure for the `Coda` measure. A jump whose
    /// target is missing (a D.S. without segno) is ignored.
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
        let last_ending = voltas.iter().map(|v| 8 - v.2.leading_zeros() as usize).max().unwrap_or(0);
        let find = |k: Marker| first.measures.iter().position(|m| m.markers.contains(&k));
        let mut order = Vec::new();
        let (mut i, mut pass, mut section_start) = (0usize, 1usize, 0usize);
        let mut jumped = false; // just came back to the start of a repeated section
        let mut active: Option<Jump> = None; // a jump has been taken: repeat signs no longer apply
        let mut taken = vec![false; n];
        let mut guard = 0usize;
        while i < n && guard < 100_000 {
            guard += 1;
            // a volta that is not for this pass is skipped
            if let Some(&(_, last, mask)) = voltas.iter().find(|v| v.0 <= i && i <= v.1) {
                let skip = if active.is_some() {
                    mask != 0 && last_ending <= 8 && mask & (1 << (last_ending - 1)) == 0
                } else {
                    mask != 0 && pass <= 8 && mask & (1 << (pass - 1)) == 0
                };
                if skip {
                    i = last + 1;
                    continue;
                }
            }
            let m = first.measures.get(i);
            if active.is_none() && m.map_or(false, |m| m.start_repeat) && !jumped {
                section_start = i;
                pass = 1;
            }
            jumped = false;
            order.push(i);
            let end = if active.is_some() { 0 } else { m.map_or(0, |m| m.end_repeat) as usize };
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
            if let Some(j) = active {
                let marks = m.map_or(&[][..], |m| &m.markers[..]);
                if j.al_fine() && marks.contains(&Marker::Fine) {
                    break;
                }
                if j.al_coda() && marks.contains(&Marker::ToCoda) {
                    if let Some(c) = find(Marker::Coda).filter(|&c| c > i) {
                        i = c;
                        continue;
                    }
                }
            }
            if let Some(j) = m.and_then(|m| m.jump).filter(|_| !taken[i]) {
                let target = if j.to_segno() { find(Marker::Segno) } else { Some(0) };
                if let Some(t) = target {
                    taken[i] = true;
                    active = Some(j);
                    pass = 1;
                    i = t;
                    continue;
                }
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
        if let Some(t) = u.transpose {
            p.transpose = t;
        }
        if let Some(t) = u.tuning {
            p.tuning = t.filter(|v| !v.is_empty() && v.len() <= 8);
        }
        if let Some(v) = u.tab_only {
            p.tab_only = v;
        }
        if p.tuning.is_none() {
            p.tab_only = false;
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

    /// Key signature (concert key: sharps positive, flats negative) in force at measure `m`.
    pub fn key_at(&self, m: usize) -> i8 {
        self.key_changes.iter().rev().find(|c| c.measure <= m).map_or(self.fifths, |c| c.fifths)
    }

    /// Set the key signature from measure `m` until the next change; `m` = 0 sets the initial key. Setting the key
    /// already in force removes the change. Notes keep their pitch (only the signature and its spelling change).
    pub fn set_key_at(&mut self, m: usize, fifths: i8) {
        let f = fifths.clamp(-7, 7);
        if m == 0 {
            self.fifths = f;
        }
        self.key_changes.retain(|c| c.measure != m && c.measure != 0);
        if m > 0 {
            self.key_changes.push(KeyChange { measure: m, fifths: f });
            self.key_changes.sort_by_key(|c| c.measure);
        }
        let mut prev = self.fifths;
        self.key_changes.retain(|c| {
            let keep = c.fifths != prev;
            prev = c.fifths;
            keep
        });
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
        // un acorde que queda fuera de su compás acortado ya no tiene dónde dibujarse
        let lens: Vec<u32> = (0..self.parts.first().map_or(0, |p| p.measures.len())).map(|i| self.measure_len(i)).collect();
        self.chords.retain(|c| lens.get(c.measure).map_or(false, |&l| c.tick < l));
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
    fn written_pitch_and_key_of_transposing_instruments() {
        let bb = Transpose { diatonic: -1, chromatic: -2 };
        let c5 = Pitch { step: 0, alter: 0, octave: 5 };
        assert_eq!(bb.to_written(c5), Pitch { step: 1, alter: 0, octave: 5 }); // concert C reads D
        assert_eq!(bb.to_sounding(Pitch { step: 1, alter: 0, octave: 5 }), c5);
        // concert B♭ reads C; concert E♭ reads F; concert F reads G
        assert_eq!(bb.to_written(Pitch { step: 6, alter: -1, octave: 4 }), Pitch { step: 0, alter: 0, octave: 5 });
        let eb_alto = Transpose { diatonic: -5, chromatic: -9 };
        assert_eq!(eb_alto.to_written(c5), Pitch { step: 5, alter: 0, octave: 5 }); // concert C reads A
        let horn = Transpose { diatonic: -4, chromatic: -7 };
        assert_eq!(horn.to_written(c5), Pitch { step: 4, alter: 0, octave: 5 }); // concert C reads G
        for t in [bb, eb_alto, horn, Transpose { diatonic: 7, chromatic: 12 }, Transpose { diatonic: -7, chromatic: -12 }] {
            for midi in 40..90 {
                for sharps in [true, false] {
                    let p = Pitch::from_midi(midi, sharps);
                    assert_eq!(t.to_sounding(t.to_written(p)).midi(), midi);
                    assert_eq!(t.to_written(p).midi(), midi - t.chromatic as i32);
                }
            }
        }
        // concert C major reads D major (2 sharps) for B♭ clarinet; concert B♭ major (-2) reads C major
        assert_eq!(bb.written_fifths(0), 2);
        assert_eq!(bb.written_fifths(-2), 0);
        assert_eq!(horn.written_fifths(0), 1);
        assert_eq!(eb_alto.written_fifths(0), 3);
        assert_eq!(eb_alto.written_fifths(5), -4); // B major reads G♯ major (8 sharps) -> A♭ major
        assert_eq!(Transpose { diatonic: 7, chromatic: 12 }.written_fifths(3), 3);
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
    fn septuplets_round_each_slot_but_add_up_exactly() {
        let mut s = score();
        // 7 eighths in the time of 4 (a half note): slots of 137 or 138 ticks, 960 in all
        s.put_note(0, 0, 0, 0, 960, vec![p(0, 4)]).unwrap();
        s.make_tuplet(0, 0, 0, 0, 7, 4).unwrap();
        let ev = s.parts[0].measures[0].events.clone();
        let g: Vec<_> = ev.iter().filter(|e| e.tuplet.is_some()).collect();
        assert_eq!(g.len(), 7);
        assert_eq!(g.iter().map(|e| e.ticks).sum::<u32>(), 960);
        assert!(g.iter().all(|e| (137..=138).contains(&e.ticks) && e.notated_ticks() == 240), "{:?}", g.iter().map(|e| (e.ticks, e.notated_ticks())).collect::<Vec<_>>());
        assert!(g.windows(2).all(|w| w[0].start + w[0].ticks == w[1].start));
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
        // notes go into the slots; the cursor advances slot by slot
        let mut at = g[2].start;
        for i in 0..3 {
            let (m, t) = s.put_note(0, 0, 0, at, 240, vec![p(i, 4)]).unwrap();
            assert_eq!(m, 0);
            at = t;
        }
        assert_eq!(at, g[5].start);
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
        // a longer value spans two slots: 480 written = 274 or 275 ticks
        let two = s.put_note(0, 0, 0, g[0].start, 480, vec![p(4, 4)]).unwrap();
        assert_eq!(two.1, g[2].start);
        let e0 = &s.parts[0].measures[0].events[0];
        assert_eq!((e0.ticks, e0.notated_ticks()), (g[2].start, 480));
        assert_eq!(total(&s.parts[0].measures[0]), 1920);
        // and the group goes back to a half note
        s.clear_tuplet(0, 0, 0, g[0].start).unwrap();
        assert_eq!(s.parts[0].measures[0].events[0].ticks, 960);
        // 7:8 over a half note: seven sixteenths in the time of eight
        let mut s = score();
        s.put_note(0, 0, 0, 0, 960, vec![p(0, 4)]).unwrap();
        s.make_tuplet(0, 0, 0, 0, 7, 8).unwrap();
        let ev = &s.parts[0].measures[0].events;
        assert!(ev[..7].iter().all(|e| e.notated_ticks() == 120) && ev[..7].iter().map(|e| e.ticks).sum::<u32>() == 960);
        // a span too short to write seven notes in it is refused
        let mut s = score();
        s.put_note(0, 0, 0, 0, 60, vec![p(0, 4)]).unwrap();
        assert!(s.make_tuplet(0, 0, 0, 0, 7, 4).is_err());
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
    fn triplet_beat_places_notes_and_rests() {
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 1);
        let p = |m| Pitch::from_midi(m, true);
        // pulso 2 (tick 480): nota, nota larga de 2 huecos
        s.put_triplet_beat(0, 0, 480, &[(0, 1, vec![p(60)], false), (1, 2, vec![p(62)], false)]).unwrap();
        let ev: Vec<(u32, u32, bool)> = s.parts[0].measures[0].events.iter().map(|e| (e.start, e.ticks, e.tuplet.is_some())).collect();
        assert_eq!(ev, vec![(0, 480, false), (480, 160, true), (640, 320, true), (960, 960, false)]);
        // un hueco vacío al principio y al final
        s.put_triplet_beat(0, 0, 1440, &[(1, 1, vec![p(64)], false)]).unwrap();
        let tail: Vec<(u32, u32, bool)> = s.parts[0].measures[0].events.iter().filter(|e| e.start >= 1440).map(|e| (e.start, e.ticks, e.content != Content::Rest)).collect();
        assert_eq!(tail, vec![(1440, 160, false), (1600, 160, true), (1760, 160, false)]);
        assert_eq!(crate::io::sounds(&s).len(), 3);
        // fuera de lugar: dos notas solapadas o un pulso que cruza la barra
        assert!(s.put_triplet_beat(0, 0, 480, &[(0, 2, vec![p(60)], false), (1, 1, vec![p(62)], false)]).is_err());
        assert!(s.put_triplet_beat(0, 0, 1700, &[(0, 1, vec![p(60)], false)]).is_err());
    }

    #[test]
    fn quintuplet_and_sextuplet_beats() {
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 1);
        let p = |m| Pitch::from_midi(m, true);
        // quintillo de semicorcheas: cinco notas de 96 ticks (5:4)
        let five: Vec<(u32, u32, Vec<Pitch>, bool)> = (0..5).map(|k| (k, 1, vec![p(60 + k as i32)], false)).collect();
        s.put_tuplet_beat(0, 0, 0, 5, &five).unwrap();
        let ev: Vec<(u32, u32)> = s.parts[0].measures[0].events.iter().take(5).map(|e| (e.start, e.ticks)).collect();
        assert_eq!(ev, vec![(0, 96), (96, 96), (192, 96), (288, 96), (384, 96)]);
        assert!(s.parts[0].measures[0].events.iter().take(5).all(|e| e.tuplet == Some(Tuplet { actual: 5, normal: 4, start: 0 }) && e.notated_ticks() == 120));
        // seisillo con una nota larga de 5 ranuras: se parte en 4 + 1 ligadas
        s.put_tuplet_beat(0, 0, 480, 6, &[(0, 5, vec![p(67)], false), (5, 1, vec![p(69)], false)]).unwrap();
        let six: Vec<(u32, u32, bool)> = s.parts[0].measures[0].events.iter().filter(|e| e.start >= 480 && e.start < 960).map(|e| (e.start, e.ticks, matches!(&e.content, Content::Note { tie: true, .. }))).collect();
        assert_eq!(six, vec![(480, 320, true), (800, 80, false), (880, 80, false)]);
        // un seisillo con una nota al final: el hueco de 5 ranuras se parte en 4 + 1 silencios
        s.put_tuplet_beat(0, 0, 960, 6, &[(5, 1, vec![p(71)], false)]).unwrap();
        let rests: Vec<u32> = s.parts[0].measures[0].events.iter().filter(|e| e.start >= 960 && e.start < 1440).map(|e| e.ticks).collect();
        assert_eq!(rests, vec![320, 80, 80]);
        assert!(s.put_tuplet_beat(0, 0, 1440, 4, &five).is_err()); // 4 ranuras no es un grupo soportado
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
    fn transposing_a_range_moves_notes_chords_and_the_key() {
        let mk = || {
            let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 3);
            for m in 0..3 {
                s.put_note(0, 0, m, 0, 960, vec![Pitch::from_midi(60, true)]).unwrap();
                s.put_note(0, 0, m, 960, 960, vec![Pitch::from_midi(64, true)]).unwrap();
            }
            s.chords = vec![chords::ChordMark { measure: 1, tick: 0, root: 0, quality: chords::Quality::Major, bass: 0, manual: true }];
            s
        };
        let notes = |s: &Score, m: usize| s.parts[0].measures[m].events.iter().filter_map(|e| if let Content::Note { pitches, .. } = &e.content { Some((pitches[0].midi(), pitches[0].step, pitches[0].alter)) } else { None }).collect::<Vec<_>>();
        // the whole piece a tone up: C major -> D major, C E -> D F sharp
        let mut s = mk();
        s.transpose_range(0, usize::MAX, 2).unwrap();
        assert_eq!((s.fifths, s.key_changes.is_empty()), (2, true));
        assert_eq!(notes(&s, 2), vec![(62, 1, 0), (66, 3, 1)]);
        assert_eq!((s.chords[0].root, s.chords[0].quality), (2, chords::Quality::Major));
        // a semitone up lands on D flat major (five flats): spelled with flats, not sharps
        let mut f = mk();
        f.transpose_range(0, usize::MAX, 1).unwrap();
        assert_eq!(f.fifths, -5);
        assert_eq!(notes(&f, 0), vec![(61, 1, -1), (65, 3, 0)]);
        // a range: only the middle measure moves, with its own key, and the key comes back after it
        let mut r = mk();
        r.transpose_range(1, 1, 7).unwrap();
        assert_eq!((r.key_at(0), r.key_at(1), r.key_at(2)), (0, 1, 0));
        assert_eq!(notes(&r, 0), vec![(60, 0, 0), (64, 2, 0)]);
        assert_eq!(notes(&r, 1), vec![(67, 4, 0), (71, 6, 0)]);
        assert_eq!(r.chords[0].root, 7);
        // undoing it is the opposite move; leaving the MIDI range or an empty range is refused
        r.transpose_range(1, 1, -7).unwrap();
        assert!(r.key_changes.is_empty() && notes(&r, 1) == notes(&mk(), 1));
        assert!(mk().transpose_range(0, 2, 70).is_err() && mk().transpose_range(5, 6, 1).is_err() && mk().transpose_range(0, 2, 0).is_err());
    }

    #[test]
    fn key_changes_are_set_merged_and_removed() {
        let mut s = Score::new("k", TimeSig { num: 4, den: 4 }, 0, 6);
        s.set_key_at(2, 2);
        s.set_key_at(4, -1);
        assert_eq!((s.key_at(0), s.key_at(1), s.key_at(2), s.key_at(3), s.key_at(4), s.key_at(5)), (0, 0, 2, 2, -1, -1));
        // repeating the key in force removes the change; the initial key moves without touching later changes
        s.set_key_at(3, 2);
        assert_eq!(s.key_changes.len(), 2);
        s.set_key_at(2, 0);
        assert_eq!(s.key_changes, vec![KeyChange { measure: 4, fifths: -1 }]);
        s.set_key_at(0, -1);
        assert!(s.key_changes.is_empty() && s.fifths == -1, "{:?}", s.key_changes);
        // an old file without the field opens
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("key_changes"));
        assert!(serde_json::from_str::<Score>(&json).is_ok());
    }

    #[test]
    fn appending_a_movement_joins_measures_and_shifts_anchors() {
        let p = |step, octave| Pitch { step, alter: 0, octave };
        let mut a = Score::new("a", TimeSig { num: 4, den: 4 }, 0, 2);
        a.put_note(0, 0, 1, 0, 1920, vec![p(0, 5)]).unwrap();
        let mut b = Score::new("b", TimeSig { num: 3, den: 4 }, 2, 2);
        b.add_part("Bajo", Clef::Bass, 0, false);
        b.put_note(0, 0, 0, 0, 1440, vec![p(1, 5)]).unwrap();
        b.put_note(1, 0, 1, 0, 1440, vec![p(2, 3)]).unwrap();
        b.set_lyric(0, 0, 0, 0, 0, "la", Syllabic::Single).unwrap();
        b.set_lyric_extend(0, 0, 0, 0, 0, Some(SpPos { measure: 1, tick: 0 })).unwrap();
        b.parts[0].spanners.push(Spanner { kind: SpannerKind::Slur, voice: 0, start: SpPos { measure: 0, tick: 0 }, end: SpPos { measure: 1, tick: 0 }, endings: 0 });
        b.chords.push(chords::ChordMark { measure: 1, tick: 0, root: 0, quality: chords::Quality::Major, bass: 0, manual: true });
        assert!(a.append(&b), "different key is reported");
        assert_eq!((a.fifths, a.key_changes.clone()), (0, vec![KeyChange { measure: 3, fifths: 2 }]), "and written as a change at the join");
        assert_eq!((a.key_at(2), a.key_at(3), a.key_at(5)), (0, 2, 2));
        assert_eq!(a.parts.len(), 2);
        // `a` ended with a spare measure after the whole note: 3 + 3 measures
        assert!(a.parts.iter().all(|p| p.measures.len() == 6));
        assert_eq!(a.time_changes, vec![TimeChange { measure: 3, time: TimeSig { num: 3, den: 4 } }]);
        assert_eq!((a.measure_len(2), a.measure_len(3)), (1920, 1440));
        // the part that did not exist before starts with rests of the right length
        assert!(a.parts[1].measures[..3].iter().all(|m| m.events.len() == 1 && m.events[0].content == Content::Rest && m.events[0].ticks == 1920));
        assert!(matches!(&a.parts[1].measures[4].events[0].content, Content::Note { .. }));
        assert_eq!(a.parts[0].spanners[0].start.measure, 3);
        assert_eq!(a.parts[0].measures[3].events[0].lyrics[0].extend_to, Some(SpPos { measure: 4, tick: 0 }));
        assert_eq!(a.chords[0].measure, 4);
        assert_eq!(a.fifths, 0);
        // same signature: no change added
        let mut c = Score::new("c", TimeSig { num: 4, den: 4 }, 0, 1);
        let d = Score::new("d", TimeSig { num: 4, den: 4 }, 0, 1);
        let before = c.parts[0].measures.len() + d.parts[0].measures.len();
        assert!(!c.append(&d) && c.time_changes.is_empty() && c.parts[0].measures.len() == before);
    }

    #[test]
    fn lyrics_are_set_replaced_and_removed() {
        let mut s = score();
        s.put_note(0, 0, 0, 0, 480, vec![p(0, 5)]).unwrap();
        s.put_note(0, 0, 0, 480, 480, vec![p(1, 5)]).unwrap();
        s.set_lyric(0, 0, 0, 0, 0, " la ", Syllabic::Begin).unwrap();
        s.set_lyric(0, 0, 0, 0, 0, "lo", Syllabic::Single).unwrap(); // replaces the verse
        s.set_lyric(0, 0, 0, 0, 1, "li", Syllabic::Single).unwrap();
        let l = &s.parts[0].measures[0].events[0].lyrics;
        assert_eq!(l.iter().map(|x| (x.verse, x.text.as_str())).collect::<Vec<_>>(), vec![(0, "lo"), (1, "li")]);
        s.set_lyric(0, 0, 0, 0, 0, "", Syllabic::Single).unwrap(); // empty removes
        assert_eq!(s.parts[0].measures[0].events[0].lyrics.len(), 1);
        assert!(s.set_lyric(0, 0, 0, 100, 0, "x", Syllabic::Single).is_err());
        s.delete_event(0, 0, 0, 0).unwrap();
        assert!(s.parts[0].measures[0].events[0].lyrics.is_empty());
        assert!(s.set_lyric(0, 0, 0, 0, 0, "x", Syllabic::Single).is_err(), "rests carry no lyrics");
        s.set_text(0, 0, 0, 0, " dolce ").unwrap(); // texts also go on rests
        assert_eq!(s.parts[0].measures[0].events[0].text.as_deref(), Some("dolce"));
        s.set_text(0, 0, 0, 0, "").unwrap();
        assert_eq!(s.parts[0].measures[0].events[0].text, None);
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
    fn jumps_unfold_into_playback_order() {
        let new = |n| Score::new("j", TimeSig { num: 4, den: 4 }, 0, n);
        // D.C. al Fine: 0 1 2 3 | 0 1 (fine on 1)
        let mut s = new(4);
        s.toggle_marker(1, Marker::Fine).unwrap();
        s.set_jump(3, Some(Jump::DcAlFine)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 2, 3, 0, 1]);
        // the same jump again removes it; Fine alone does nothing
        s.set_jump(3, Some(Jump::DcAlFine)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 2, 3]);
        // plain D.C.: everything again, once
        s.set_jump(3, Some(Jump::Dc)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 2, 3, 0, 1, 2, 3]);
        // D.S. al Coda: segno at 1, to-coda at 2, jump at 4, coda at 5
        let mut s = new(6);
        s.toggle_marker(1, Marker::Segno).unwrap();
        s.toggle_marker(2, Marker::ToCoda).unwrap();
        s.toggle_marker(5, Marker::Coda).unwrap();
        s.set_jump(4, Some(Jump::DsAlCoda)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 2, 3, 4, 1, 2, 5]);
        // D.S. without segno is ignored; so is al Coda without coda
        let mut s = new(3);
        s.set_jump(2, Some(Jump::Ds)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 2]);
        let mut s = new(3);
        s.toggle_marker(0, Marker::ToCoda).unwrap();
        s.set_jump(2, Some(Jump::DcAlCoda)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 2, 0, 1, 2]);
        // repeats are not taken again after the jump, and only the last ending is played
        let mut s = new(5);
        s.toggle_repeat(0, true).unwrap();
        s.toggle_repeat(2, false).unwrap();
        let v = |m: usize, mask: u8| Spanner { kind: SpannerKind::Volta, voice: 0, start: pos(m, 0), end: pos(m, 1920), endings: mask };
        s.toggle_repeat(1, false).unwrap(); // measure 1 closes the repeat after the first ending
        s.parts[0].measures[2].end_repeat = 0;
        s.add_spanner(0, v(1, 0b01)).unwrap();
        s.add_spanner(0, v(2, 0b10)).unwrap();
        s.set_jump(4, Some(Jump::Dc)).unwrap();
        assert_eq!(s.playback_order(), vec![0, 1, 0, 2, 3, 4, 0, 2, 3, 4]);
        assert!(s.set_jump(9, Some(Jump::Dc)).is_err());
        assert!(s.toggle_marker(9, Marker::Fine).is_err());
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

