//! MuseScore writer: produces `.mscx` / `.mscz` files in the MuseScore 4 format.
//!
//! The score model is written in full. Markup that was kept verbatim by the importer
//! (`Event::extras`, `Measure::extras`, `Part::ms`, `Score::ms_style`) is copied back so that
//! dynamics, articulations, lyrics, repeats, instruments and style survive a round trip.

use crate::*;
use std::fmt::Write as _;
use std::io::Write as _;

/// Drum kit instrument definition (MuseScore's own, from its Big Band template).
const DRUMSET: &str = include_str!("drumset.xml");

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

/// Fraction of a whole note, reduced ("1/4", "-3/8").
fn frac(ticks: i64) -> String {
    if ticks == 0 {
        return "0/1".into();
    }
    let g = gcd(ticks.abs(), 1920);
    format!("{}/{}", ticks / g, 1920 / g)
}

fn duration_type(ticks: u32) -> Option<(&'static str, u32)> {
    const TYPES: [(&str, u32); 6] = [("whole", 1920), ("half", 960), ("quarter", 480), ("eighth", 240), ("16th", 120), ("32nd", 60)];
    for (name, v) in TYPES.iter().chain([("64th", 30u32)].iter()) {
        if ticks == *v {
            return Some((name, 0));
        }
        if ticks == v + v / 2 {
            return Some((name, 1));
        }
    }
    None
}

/// Tonal pitch class (14 = C, one more per fifth up).
fn tpc(p: &Pitch) -> i32 {
    const BASE: [i32; 7] = [0, 2, 4, -1, 1, 3, 5]; // C D E F G A B on the line of fifths
    14 + BASE[p.step as usize % 7] + 7 * p.alter as i32
}

fn instrument_id(program: u8) -> &'static str {
    match program {
        0..=7 => "piano",
        16..=20 => "organ",
        24 => "guitar-nylon",
        25 => "guitar-steel",
        32..=39 => "bass-guitar",
        40 => "violin",
        41 => "viola",
        42 => "violoncello",
        43 => "contrabass",
        46 => "harp",
        48..=51 => "strings-group",
        52..=54 => "voice",
        56 => "trumpet",
        57 => "trombone",
        58 => "tuba",
        60 => "horn",
        65 | 66 => "alto-saxophone",
        68 => "oboe",
        70 => "bassoon",
        71 => "clarinet",
        73 => "flute",
        _ => "piano",
    }
}

fn artic_subtype(a: Artic) -> &'static str {
    match a {
        Artic::Staccato => "articStaccatoAbove",
        Artic::Staccatissimo => "articStaccatissimoAbove",
        Artic::Tenuto => "articTenutoAbove",
        Artic::Accent => "articAccentAbove",
        Artic::Marcato => "articMarcatoAbove",
        Artic::Portato => "articTenutoStaccatoAbove",
        Artic::Mordent => "ornamentMordent",
        Artic::ShortTrill => "ornamentShortTrill",
        Artic::Turn => "ornamentTurn",
        Artic::InvertedTurn => "ornamentTurnInverted",
    }
}

fn short_name(name: &str) -> String {
    let base = name.split(" (").next().unwrap_or(name);
    let s: String = base.chars().take(4).collect();
    format!("{s}.")
}

fn generated_instrument(p: &Part, name: &str) -> String {
    if p.percussion {
        return DRUMSET.replace("<longName>Drum Kit</longName>", &format!("<longName>{}</longName>", esc(name)))
            .replace("<trackName>Drum Kit</trackName>", &format!("<trackName>{}</trackName>", esc(name)));
    }
    let strings = p.tuning.as_ref().map_or(String::new(), |t| {
        let list: String = t.iter().map(|m| format!("
          <string>{m}</string>")).collect();
        format!("
        <StringData>
          <frets>25</frets>{list}
          </StringData>")
    });
    let vol = if p.volume != 100 { format!("\n          <controller ctrl=\"7\" value=\"{}\"/>", p.volume) } else { String::new() };
    format!(
        "<Instrument id=\"{id}\">\n        <longName>{n}</longName>\n        <shortName>{s}</shortName>\n        <trackName>{n}</trackName>\n        <minPitchP>0</minPitchP>\n        <maxPitchP>127</maxPitchP>\n        <minPitchA>0</minPitchA>\n        <maxPitchA>127</maxPitchA>{tr}{strings}\n        <Channel>\n          <program value=\"{prog}\"/>{vol}\n          <synti>Fluid</synti>\n          </Channel>\n        </Instrument>",
        strings = strings,
        tr = p.transpose.map_or(String::new(), |t| format!("
        <transposeDiatonic>{}</transposeDiatonic>
        <transposeChromatic>{}</transposeChromatic>", t.diatonic, t.chromatic)),
        id = instrument_id(p.program),
        n = esc(name),
        s = esc(&short_name(name)),
        prog = p.program,
        vol = vol
    )
}

fn generated_staff(p: &Part) -> String {
    if p.tab_staff {
        let lines = p.tuning.as_ref().map_or(6, |t| t.len());
        return format!("<StaffType group=\"tablature\">
          <name>tab{lines}StrCommon</name>
          <lines>{lines}</lines>
          <lineDistance>1.5</lineDistance>
          <durations>0</durations>
          <durationFontName>MuseScore Tab Modern</durationFontName>
          <durationFontSize>15</durationFontSize>
          <durationFontY>0</durationFontY>
          <fretUseTextStyle>1</fretUseTextStyle>
          <fretTextStyle>tab_fret_number</fretTextStyle>
          <linesThrough>0</linesThrough>
          <minimStyle>1</minimStyle>
          <onLines>1</onLines>
          <showRests>1</showRests>
          <stemsDown>1</stemsDown>
          <stemsThrough>0</stemsThrough>
          <upsideDown>0</upsideDown>
          <showTabFingering>1</showTabFingering>
          <useNumbers>1</useNumbers>
          </StaffType>
        <defaultClef>TAB</defaultClef>");
    }
    if p.percussion {
        "<StaffType group=\"percussion\">\n          <name>perc5Line</name>\n          <keysig>0</keysig>\n          </StaffType>\n        <defaultClef>PERC</defaultClef>".into()
    } else {
        let clef = if p.clef == Clef::Bass { "\n        <defaultClef>F</defaultClef>" } else { "" };
        format!("<StaffType group=\"pitched\">\n          <name>stdNormal</name>\n          </StaffType>{clef}")
    }
}

/// True when the part still matches the MuseScore data it was imported with.
fn origin_valid(p: &Part) -> Option<&MsOrigin> {
    p.ms.as_ref().filter(|o| o.name == p.name && o.program == p.program && o.clef == p.clef && o.percussion == p.percussion && o.transpose == p.transpose && o.tuning == p.tuning)
}

/// Groups of consecutive parts that share one MuseScore `<Part>` (a piano's two staves).
fn part_groups(score: &Score) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, p) in score.parts.iter().enumerate() {
        let joins = groups.last().map_or(false, |g| {
            let prev = &score.parts[*g.last().unwrap()];
            match (&prev.ms, &p.ms) {
                (Some(a), Some(b)) => a.group == b.group,
                (None, None) => {
                    // staves created by NovaScore or by a MusicXML import: "Piano", "Piano (2)"
                    let first = &score.parts[g[0]];
                    p.name == format!("{} ({})", first.name, g.len() + 1) && p.program == first.program && !p.percussion && !first.percussion
                }
                _ => false,
            }
        });
        if joins {
            groups.last_mut().unwrap().push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    groups
}

/// Slur and hairpin markers attached to one event (locations are relative to that event).
#[derive(Default, Clone)]
struct Marks {
    slur_start: Vec<(i32, i64)>,
    slur_end: Vec<(i32, i64)>,
    /// (kind, volta endings, volta closed by a repeat, location of the end)
    hp_start: Vec<(SpannerKind, u8, bool, i32, i64)>,
    hp_end: Vec<(SpannerKind, i32, i64)>,
    /// Octaves between sounding and written pitch of this event (octave lines).
    oct: i32,
    /// Melisma lengths in ticks past the end of this event, per verse.
    lyric_ext: Vec<(u8, u32)>,
    /// Transposition of the part: notes also carry the written spelling (`tpc2`).
    transpose: Option<Transpose>,
    /// Tablature position of each pitch (ascending): string (MuseScore counts from 0 at the highest) and fret.
    tab: Vec<Option<(u8, u8)>>,
}

/// Move a spanner's ends onto real event boundaries of its voice. Slurs need notes at both
/// ends; a hairpin may end on the closing barline. Returns None when it cannot be placed.
pub(crate) fn snap_spanner(score: &Score, part: &Part, sp: &Spanner) -> Option<(SpPos, SpPos)> {
    let voice = sp.voice as usize;
    let event_at = |m: usize, tick: u32, use_end: bool| -> Option<(u32, &Event)> {
        let evs = part.measures.get(m)?.voice(voice)?;
        let e = evs.iter().find(|e| e.start <= tick && tick < e.start + e.ticks)?;
        Some((if use_end && tick > e.start { e.start + e.ticks } else { e.start }, e))
    };
    let is_note = |e: &Event| matches!(e.content, Content::Note { .. });
    match sp.kind {
        SpannerKind::Slur => {
            let (a, ea) = event_at(sp.start.measure, sp.start.tick, false)?;
            let (b, eb) = event_at(sp.end.measure, sp.end.tick, false)?;
            if !is_note(ea) || !is_note(eb) {
                return None;
            }
            let (a, b) = (SpPos { measure: sp.start.measure, tick: a }, SpPos { measure: sp.end.measure, tick: b });
            (a < b).then_some((a, b))
        }
        _ => {
            let (a, _) = event_at(sp.start.measure, sp.start.tick, false)?;
            let mut end = sp.end;
            if end.tick == 0 && end.measure > 0 {
                end = SpPos { measure: end.measure - 1, tick: score.measure_len(end.measure - 1) };
            }
            let len = score.measure_len(end.measure);
            let b = if end.tick >= len { len } else { event_at(end.measure, end.tick, true)?.0 };
            let (a, b) = (SpPos { measure: sp.start.measure, tick: a }, SpPos { measure: end.measure, tick: b });
            (a < b).then_some((a, b))
        }
    }
}

/// Markers for the events of one voice in one measure, plus end markers on the closing barline.
fn compute_marks(score: &Score, part: &Part, voice: usize, mi: usize, evs: &[Event]) -> (Vec<Marks>, Vec<(SpannerKind, i32, i64)>) {
    let mut marks = vec![Marks::default(); evs.len()];
    for m in &mut marks {
        m.transpose = part.transpose;
    }
    if part.tuning.is_some() {
        if let Some(pi) = score.parts.iter().position(|q| std::ptr::eq(q, part)) {
            let positions = score.tab_positions(pi, voice);
            for (i, e) in evs.iter().enumerate() {
                if let Some(v) = positions.get(&(mi, e.start)) {
                    marks[i].tab = v.iter().map(|f| f.map(|f| (f.string - 1, f.fret))).collect();
                }
            }
        }
    }
    // Octave lines act on the whole staff, whatever voice they were written in.
    for sp in part.spanners.iter().filter(|s| s.kind.octaves() != 0) {
        for (i, e) in evs.iter().enumerate() {
            let pos = SpPos { measure: mi, tick: e.start };
            if pos >= sp.start && pos < sp.end {
                marks[i].oct += sp.kind.octaves();
            }
        }
    }
    for (i, e) in evs.iter().enumerate() {
        for l in &e.lyrics {
            if let Some(end) = l.extend_to {
                let starts = score.measure_starts(end.measure.max(mi) + 1);
                let (from, to) = (starts[mi] + e.start + e.ticks, starts[end.measure] + end.tick);
                if to > from {
                    marks[i].lyric_ext.push((l.verse, to - from));
                }
            }
        }
    }
    let mut tail = Vec::new();
    let mlen = score.measure_len(mi);
    for sp in part.spanners.iter().filter(|s| s.voice as usize == voice) {
        let Some((a, b)) = snap_spanner(score, part, sp) else { continue };
        let (dm, dt) = (b.measure as i32 - a.measure as i32, b.tick as i64 - a.tick as i64);
        if a.measure == mi {
            if let Some(i) = evs.iter().position(|e| e.start == a.tick) {
                match sp.kind {
                    SpannerKind::Slur => marks[i].slur_start.push((dm, dt)),
                    k => marks[i].hp_start.push((k, sp.endings, part.measures.get(b.measure).map_or(false, |m| m.end_repeat > 0), dm, dt)),
                }
            }
        }
        if b.measure == mi {
            match sp.kind {
                SpannerKind::Slur => {
                    if let Some(i) = evs.iter().position(|e| e.start == b.tick) {
                        marks[i].slur_end.push((-dm, -dt));
                    }
                }
                k if b.tick >= mlen => tail.push((k, -dm, -dt)),
                _ => {
                    if let Some(i) = evs.iter().position(|e| e.start == b.tick) {
                        marks[i].hp_end.push((sp.kind, -dm, -dt));
                    }
                }
            }
        }
    }
    (marks, tail)
}

fn write_slur_marker(out: &mut String, indent: &str, start: bool, loc: (i32, i64)) {
    let _ = writeln!(out, "{indent}<Spanner type=\"Slur\">");
    if start {
        let _ = write!(out, "{indent}  <Slur>\n{indent}    </Slur>\n{indent}  <next>\n{indent}    ");
        write_location(out, loc.0, loc.1);
        let _ = writeln!(out, "\n{indent}    </next>");
    } else {
        let _ = write!(out, "{indent}  <prev>\n{indent}    ");
        write_location(out, loc.0, loc.1);
        let _ = writeln!(out, "\n{indent}    </prev>");
    }
    let _ = writeln!(out, "{indent}  </Spanner>");
}

/// MuseScore element name and body of a line spanner.
fn line_markup(k: SpannerKind, endings: u8, closed: bool) -> (&'static str, String) {
    match k {
        SpannerKind::Trill => ("Trill", "<subtype>trill</subtype>".into()),
        SpannerKind::Volta => {
            let nums: Vec<String> = (1..=8).filter(|n| endings & (1 << (n - 1)) != 0).map(|n| n.to_string()).collect();
            ("Volta", format!("<endHookType>{}</endHookType>\n<beginText>{}</beginText>\n<endings>{}</endings>", u8::from(closed), nums.join(", "), nums.join(",")))
        }
        SpannerKind::Crescendo => ("HairPin", "<subtype>0</subtype>".into()),
        SpannerKind::Decrescendo => ("HairPin", "<subtype>1</subtype>".into()),
        SpannerKind::Ottava8va => ("Ottava", "<subtype>8va</subtype>".into()),
        SpannerKind::Ottava8vb => ("Ottava", "<subtype>8vb</subtype>".into()),
        SpannerKind::Ottava15ma => ("Ottava", "<subtype>15ma</subtype>".into()),
        SpannerKind::Ottava15mb => ("Ottava", "<subtype>15mb</subtype>".into()),
        _ => ("Pedal", String::new()),
    }
}

fn write_hairpin_marker(out: &mut String, indent: &str, k: SpannerKind, ex: (u8, bool), start: bool, loc: (i32, i64)) {
    let (name, body) = line_markup(k, ex.0, ex.1);
    let _ = writeln!(out, "{indent}<Spanner type=\"{name}\">");
    if start {
        let _ = write!(out, "{indent}  <{name}>\n{indent}    {body}\n{indent}    </{name}>\n{indent}  <next>\n{indent}    ");
        write_location(out, loc.0, loc.1);
        let _ = writeln!(out, "\n{indent}    </next>");
    } else {
        let _ = write!(out, "{indent}  <prev>\n{indent}    ");
        write_location(out, loc.0, loc.1);
        let _ = writeln!(out, "\n{indent}    </prev>");
    }
    let _ = writeln!(out, "{indent}  </Spanner>");
}

fn write_location(out: &mut String, measures: i32, ticks: i64) {
    out.push_str("<location>");
    if measures != 0 {
        let _ = write!(out, "<measures>{measures}</measures>");
    }
    if ticks != 0 {
        let _ = write!(out, "<fractions>{}</fractions>", frac(ticks));
    }
    out.push_str("</location>");
}

fn has_pitch(e: Option<&Event>, midi: i32) -> bool {
    matches!(e.map(|x| &x.content), Some(Content::Note { pitches, .. }) if pitches.iter().any(|q| q.midi() == midi))
}

/// One voice of one measure.
#[allow(clippy::too_many_arguments)]
fn write_voice(out: &mut String, evs: &[Event], voice: usize, mlen: u32, prev_last: Option<&Event>, next_first: Option<&Event>, indent: &str, marks: &[Marks], tail: &[(SpannerKind, i32, i64)]) {
    let mut i = 0;
    while i < evs.len() {
        let e = &evs[i];
        // Tuplet group starting here?
        let mut group_end = i;
        let mut opened = false;
        if let Some(t) = e.tuplet {
            if i == 0 || evs[i - 1].tuplet != Some(t) {
                group_end = i;
                while group_end + 1 < evs.len() && evs[group_end + 1].tuplet == Some(t) {
                    group_end += 1;
                }
                let span: u32 = evs[i..=group_end].iter().map(|x| x.ticks).sum();
                if let Some((base, 0)) = duration_type(span / t.normal as u32).filter(|_| span % t.normal as u32 == 0) {
                    let _ = writeln!(
                        out,
                        "{indent}<Tuplet>\n{indent}  <normalNotes>{}</normalNotes>\n{indent}  <actualNotes>{}</actualNotes>\n{indent}  <baseNote>{base}</baseNote>\n{indent}  <Number>\n{indent}    <style>Tuplet</style>\n{indent}    <text>{}</text>\n{indent}    </Number>\n{indent}  </Tuplet>",
                        t.normal, t.actual, t.actual
                    );
                    opened = true;
                }
            }
        }
        write_event(out, evs, i, voice, mlen, prev_last, next_first, indent, &marks[i]);
        if opened {
            // find the group end again (events of this group are consecutive)
            let t = e.tuplet.unwrap();
            let mut j = i;
            while j + 1 < evs.len() && evs[j + 1].tuplet == Some(t) {
                j += 1;
            }
            for k in i + 1..=j {
                write_event(out, evs, k, voice, mlen, prev_last, next_first, indent, &marks[k]);
            }
            let _ = writeln!(out, "{indent}<endTuplet/>");
            i = j + 1;
        } else {
            i += 1;
        }
        let _ = group_end;
    }
    for &(k, m, t) in tail {
        write_hairpin_marker(out, indent, k, (0, false), false, (m, t));
    }
    // Markup after the last chord
    if let Some(last) = evs.last() {
        for x in last.extras.iter().filter(|x| x.place == ExtraPlace::After) {
            let _ = writeln!(out, "{indent}{}", x.xml.trim());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_event(out: &mut String, evs: &[Event], i: usize, voice: usize, mlen: u32, prev_last: Option<&Event>, next_first: Option<&Event>, indent: &str, mk: &Marks) {
    let e = &evs[i];
    for &(k, m, t) in &mk.hp_end {
        write_hairpin_marker(out, indent, k, (0, false), false, (m, t));
    }
    for &(k, endings, closed, m, t) in &mk.hp_start {
        write_hairpin_marker(out, indent, k, (endings, closed), true, (m, t));
    }
    if let Some(d) = e.dynamic {
        let _ = writeln!(out, "{indent}<Dynamic>\n{indent}  <subtype>{}</subtype>\n{indent}  </Dynamic>", d.name());
    }
    if let Some(t) = &e.text {
        let _ = writeln!(out, "{indent}<StaffText>
{indent}  <text>{}</text>
{indent}  </StaffText>", t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"));
    }
    for x in e.extras.iter().filter(|x| x.place == ExtraPlace::Before) {
        let _ = writeln!(out, "{indent}{}", x.xml.trim());
    }
    let (dtype, dots) = duration_type(e.notated_ticks()).unwrap_or(("quarter", 0));
    let dots_xml = if dots > 0 { format!("{indent}  <dots>{dots}</dots>\n") } else { String::new() };
    match &e.content {
        Content::Rest => {
            if voice == 0 && e.start == 0 && e.ticks == mlen && e.tuplet.is_none() {
                let _ = writeln!(out, "{indent}<Rest>\n{indent}  <durationType>measure</durationType>\n{indent}  <duration>{}</duration>\n{indent}  </Rest>", frac(mlen as i64));
            } else {
                let _ = writeln!(out, "{indent}<Rest>");
                for x in e.extras.iter().filter(|x| x.place == ExtraPlace::Inside) {
                    let _ = writeln!(out, "{indent}  {}", x.xml.trim());
                }
                let _ = write!(out, "{dots_xml}{indent}  <durationType>{dtype}</durationType>\n{indent}  </Rest>\n");
            }
        }
        Content::Note { pitches, tie } => {
            let prev = if i > 0 { Some(&evs[i - 1]) } else { prev_last };
            let next = evs.get(i + 1).or(next_first);
            let next_in_measure = i + 1 < evs.len();
            let _ = writeln!(out, "{indent}<Chord>");
            let _ = write!(out, "{dots_xml}{indent}  <durationType>{dtype}</durationType>\n");
            for a in &e.articulations {
                let tag = if a.is_ornament() { "Ornament" } else { "Articulation" };
                let _ = writeln!(out, "{indent}  <{tag}>\n{indent}    <subtype>{}</subtype>\n{indent}    </{tag}>", artic_subtype(*a));
            }
            for l in &e.lyrics {
                let _ = write!(out, "{indent}  <Lyrics>
");
                if l.verse > 0 {
                    let _ = writeln!(out, "{indent}    <no>{}</no>", l.verse);
                }
                match l.syllabic {
                    Syllabic::Single => {}
                    Syllabic::Begin => {
                        let _ = writeln!(out, "{indent}    <syllabic>begin</syllabic>");
                    }
                    Syllabic::Middle => {
                        let _ = writeln!(out, "{indent}    <syllabic>middle</syllabic>");
                    }
                    Syllabic::End => {
                        let _ = writeln!(out, "{indent}    <syllabic>end</syllabic>");
                    }
                }
                if let Some(&(_, t)) = mk.lyric_ext.iter().find(|x| x.0 == l.verse) {
                    let g = {
                        fn gcd(a: u32, b: u32) -> u32 {
                            if b == 0 { a } else { gcd(b, a % b) }
                        }
                        gcd(t, PPQ * 4)
                    };
                    let _ = writeln!(out, "{indent}    <ticks_f>{}/{}</ticks_f>", t / g, PPQ * 4 / g);
                }
                let _ = writeln!(out, "{indent}    <text>{}</text>
{indent}    </Lyrics>", l.text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"));
            }
            for &loc in &mk.slur_end {
                write_slur_marker(out, &format!("{indent}  "), false, loc);
            }
            for &loc in &mk.slur_start {
                write_slur_marker(out, &format!("{indent}  "), true, loc);
            }
            for x in e.extras.iter().filter(|x| x.place == ExtraPlace::Inside) {
                let _ = writeln!(out, "{indent}  {}", x.xml.trim());
            }
            for (pk, p) in pitches.iter().enumerate() {
                let midi = p.midi();
                // Under an octave line MuseScore stores the written pitch.
                let written = midi - 12 * mk.oct;
                let _ = writeln!(out, "{indent}  <Note>");
                for x in e.extras.iter().filter(|x| x.place == ExtraPlace::Note(written)) {
                    let _ = writeln!(out, "{indent}    {}", x.xml.trim());
                }
                // Tie to the next event (same pitch), and from the previous one.
                let tie_next = *tie && has_pitch(next, midi);
                let tie_prev = matches!(prev.map(|x| &x.content), Some(Content::Note { tie: true, .. })) && has_pitch(prev, midi);
                if tie_next || tie_prev {
                    let _ = writeln!(out, "{indent}    <Spanner type=\"Tie\">");
                    if tie_next {
                        let (m, t) = if next_in_measure { (0, next.unwrap().start as i64 - e.start as i64) } else { (1, -(e.start as i64)) };
                        let _ = write!(out, "{indent}      <Tie>\n{indent}        </Tie>\n{indent}      <next>\n{indent}        ");
                        write_location(out, m, t);
                        let _ = writeln!(out, "\n{indent}        </next>");
                    }
                    if tie_prev {
                        // previous event is in this measure unless this is the first one
                        let pstart = prev.map_or(0, |x| x.start as i64);
                        let (m, t) = if i > 0 { (0, pstart - e.start as i64) } else { (-1, pstart - e.start as i64) };
                        let _ = write!(out, "{indent}      <prev>\n{indent}        ");
                        write_location(out, m, t);
                        let _ = writeln!(out, "\n{indent}        </prev>");
                    }
                    let _ = writeln!(out, "{indent}      </Spanner>");
                }
                let tpc2 = mk.transpose.map_or(String::new(), |t| format!("\n{indent}    <tpc2>{}</tpc2>", tpc(&t.to_written(*p))));
                let fret = mk.tab.get(pk).copied().flatten().map_or(String::new(), |(string, fret)| format!("\n{indent}    <fret>{fret}</fret>\n{indent}    <string>{string}</string>"));
                let _ = writeln!(out, "{indent}    <pitch>{written}</pitch>\n{indent}    <tpc>{}</tpc>{tpc2}{fret}\n{indent}    </Note>", tpc(p));
            }
            let _ = writeln!(out, "{indent}  </Chord>");
        }
    }
}

/// Write the score as a MuseScore 4 `.mscx` document.
/// MuseScore shows a tablature as a second staff of the part: a copy of the notes with a tablature staff type.
/// Returns the score with that twin after every part that has a tuning.
fn with_tab_twins(score: &Score) -> Score {
    let mut out = score.clone();
    out.parts.clear();
    for p in &score.parts {
        out.parts.push(p.clone());
        if p.tuning.is_some() && !p.percussion {
            let mut twin = p.clone();
            twin.name = format!("{} (2)", p.name);
            twin.ms = None;
            twin.tab_staff = true;
            twin.spanners.clear();
            for m in &mut twin.measures {
                m.extras.clear();
                for e in m.events.iter_mut().chain(m.voices.iter_mut().flatten()) {
                    e.extras.clear();
                    e.dynamic = None;
                    e.articulations.clear();
                    e.lyrics.clear();
                    e.text = None;
                }
            }
            out.parts.push(twin);
        }
    }
    out
}

pub fn export_mscx(score: &Score) -> String {
    let twins;
    let score = if score.parts.iter().any(|p| p.tuning.is_some() && !p.percussion) {
        twins = with_tab_twins(score);
        &twins
    } else {
        score
    };
    let n_measures = score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<museScore version=\"4.60\">\n  <Score>\n");
    x.push_str("    <LayerTag id=\"0\" tag=\"default\"></LayerTag>\n    <currentLayer>0</currentLayer>\n    <Division>480</Division>\n");
    x.push_str("    <showInvisible>1</showInvisible>\n    <showUnprintable>1</showUnprintable>\n    <showFrames>1</showFrames>\n    <showMargins>0</showMargins>\n");
    for (name, value) in [
        ("arranger", ""),
        ("composer", score.composer.as_str()),
        ("copyright", ""),
        ("lyricist", ""),
        ("movementNumber", ""),
        ("movementTitle", ""),
        ("source", ""),
        ("translator", ""),
        ("workNumber", ""),
        ("workTitle", score.title.as_str()),
    ] {
        let _ = writeln!(x, "    <metaTag name=\"{name}\">{}</metaTag>", esc(value));
    }

    // ---- parts
    let groups = part_groups(score);
    for g in &groups {
        x.push_str("    <Part>\n");
        for &pi in g {
            let p = &score.parts[pi];
            let inner = origin_valid(p).map(|o| o.staff_xml.clone()).unwrap_or_else(|| generated_staff(p));
            let _ = writeln!(x, "      <Staff id=\"{}\">{}</Staff>", pi + 1, inner);
        }
        let first = &score.parts[g[0]];
        let name = first.name.clone();
        let _ = writeln!(x, "      <trackName>{}</trackName>", esc(&name));
        let inst = origin_valid(first).map(|o| o.instrument_xml.clone()).unwrap_or_else(|| generated_instrument(first, &name));
        let _ = writeln!(x, "      {}", inst.trim());
        x.push_str("      </Part>\n");
    }

    // ---- staves
    let starts_of = |mi: usize| score.time_changes.iter().find(|c| c.measure == mi).map(|c| c.time);
    for (pi, part) in score.parts.iter().enumerate() {
        let _ = writeln!(x, "    <Staff id=\"{}\">", pi + 1);
        if pi == 0 {
            let _ = write!(x, "      <VBox>\n        <height>10</height>\n        <Text>\n          <style>title</style>\n          <text>{}</text>\n          </Text>\n", esc(&score.title));
            if !score.composer.is_empty() {
                let _ = write!(x, "        <Text>\n          <style>composer</style>\n          <text>{}</text>\n          </Text>\n", esc(&score.composer));
            }
            x.push_str("        </VBox>\n");
        }
        for mi in 0..n_measures {
            let Some(m) = part.measures.get(mi) else { break };
            let mlen = score.measure_len(mi);
            x.push_str("      <Measure>\n");
            if m.start_repeat {
                x.push_str("        <startRepeat/>\n");
            }
            if m.end_repeat > 0 {
                let _ = writeln!(x, "        <endRepeat>{}</endRepeat>", m.end_repeat);
            }
            for frag in &m.extras {
                let _ = writeln!(x, "        {}", frag.trim());
            }
            if pi == 0 {
                for mk in &m.markers {
                    let (style, subtype, text) = match mk {
                        Marker::Segno => ("repeat_left", "segno", "<sym>segno</sym>"),
                        Marker::Coda => ("repeat_left", "codab", "<sym>coda</sym>"),
                        Marker::ToCoda => ("repeat_right", "coda", "To Coda"),
                        Marker::Fine => ("repeat_right", "fine", "Fine"),
                    };
                    let _ = writeln!(x, "        <Marker>
          <style>{style}</style>
          <subtype>{subtype}</subtype>
          <text>{text}</text>
          <label>{subtype}</label>
          </Marker>");
                }
                if let Some(j) = m.jump {
                    let to = if j.to_segno() { "segno" } else { "start" };
                    let until = if j.al_fine() { "fine" } else if j.al_coda() { "coda" } else { "end" };
                    let cont = if j.al_coda() { "codab" } else { "" };
                    let _ = writeln!(x, "        <Jump>
          <style>repeat_right</style>
          <text>{}</text>
          <jumpTo>{to}</jumpTo>
          <playUntil>{until}</playUntil>
          <continueAt>{cont}</continueAt>
          </Jump>", j.text());
                }
            }
            for v in 0..m.voice_count() {
                let Some(evs) = m.voice(v) else { continue };
                // Cifrado: un <Harmony> antes del evento que suena en el tick del acorde (el primer pentagrama, primera voz).
                let with_chords: Vec<Event>;
                let evs: &[Event] = if pi == 0 && v == 0 && score.chords.iter().any(|c| c.measure == mi) {
                    let mut owned = evs.to_vec();
                    // MuseScore ancla el cifrado a una nota: un acorde a mitad de nota larga se adelanta al inicio de la
                    // nota y, si caen varios en la misma, se queda el primero.
                    for c in score.chords.iter().filter(|c| c.measure == mi) {
                        if let Some(e) = owned.iter_mut().find(|e| e.start <= c.tick && c.tick < e.start + e.ticks) {
                            if e.extras.iter().any(|x| x.xml.starts_with("<Harmony>") && x.place == ExtraPlace::Before) {
                                continue;
                            }
                            e.extras.insert(0, Extra { place: ExtraPlace::Before, xml: c.mscx_harmony(score.fifths) });
                        }
                    }
                    with_chords = owned;
                    &with_chords
                } else {
                    evs
                };
                x.push_str("        <voice>\n");
                if v == 0 {
                    if mi == 0 || starts_of(mi).is_some() {
                        let t = score.time_at(mi);
                        let _ = writeln!(x, "          <TimeSig>\n            <sigN>{}</sigN>\n            <sigD>{}</sigD>\n            </TimeSig>", t.num, t.den);
                    }
                    if mi == 0 {
                        if !part.percussion {
                            let actual = part.transpose.map_or(String::new(), |t| format!("\n            <actualKey>{}</actualKey>", t.written_fifths(score.fifths)));
                            let _ = writeln!(x, "          <KeySig>\n            <concertKey>{}</concertKey>{actual}\n            </KeySig>", score.fifths);
                        }
                        if pi == 0 {
                            let _ = writeln!(
                                x,
                                "          <Tempo>\n            <tempo>{:.5}</tempo>\n            <followText>1</followText>\n            <text><sym>metNoteQuarterUp</sym> = {}</text>\n            </Tempo>",
                                score.tempo as f64 / 60.0,
                                score.tempo
                            );
                        }
                    }
                }
                let prev_last = if mi > 0 { part.measures[mi - 1].voice(v).and_then(|e| e.last()) } else { None };
                let next_first = part.measures.get(mi + 1).and_then(|n| n.voice(v)).and_then(|e| e.first());
                let (marks, tail) = compute_marks(score, part, v, mi, evs);
                write_voice(&mut x, evs, v, mlen, prev_last, next_first, "          ", &marks, &tail);
                x.push_str("          </voice>\n");
            }
            x.push_str("        </Measure>\n");
        }
        x.push_str("      </Staff>\n");
    }
    x.push_str("    </Score>\n  </museScore>\n");
    x
}

/// Write the score as a `.mscz` archive (a zip with the `.mscx`, its style and a container file).
pub fn export_mscz(score: &Score, stem: &str) -> Result<Vec<u8>, String> {
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') { c } else { '_' }).collect();
    let stem = if stem.trim().is_empty() { "score".to_string() } else { stem };
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let io_err = |e: std::io::Error| e.to_string();
        let zip_err = |e: zip::result::ZipError| e.to_string();
        let mut container = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<container>\n  <rootfiles>\n");
        if score.ms_style.is_some() {
            container.push_str("    <rootfile full-path=\"score_style.mss\"/>\n");
        }
        let _ = writeln!(container, "    <rootfile full-path=\"{}.mscx\"/>\n    </rootfiles>\n  </container>", esc(&stem));
        z.start_file("META-INF/container.xml", opts).map_err(zip_err)?;
        z.write_all(container.as_bytes()).map_err(io_err)?;
        if let Some(style) = &score.ms_style {
            z.start_file("score_style.mss", opts).map_err(zip_err)?;
            z.write_all(style.as_bytes()).map_err(io_err)?;
        }
        z.start_file(format!("{stem}.mscx"), opts).map_err(zip_err)?;
        z.write_all(export_mscx(score).as_bytes()).map_err(io_err)?;
        z.finish().map_err(zip_err)?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mscx::{import_mscx, import_mscz};

    fn p(step: u8, octave: i8) -> Pitch {
        Pitch { step, alter: 0, octave }
    }

    #[test]
    fn helper_formats() {
        assert_eq!(frac(480), "1/4");
        assert_eq!(frac(-720), "-3/8");
        assert_eq!(frac(1440), "3/4");
        assert_eq!(tpc(&p(0, 4)), 14); // C
        assert_eq!(tpc(&Pitch { step: 3, alter: 1, octave: 4 }), 20); // F#
        assert_eq!(tpc(&Pitch { step: 6, alter: -1, octave: 4 }), 12); // Bb
        assert_eq!(duration_type(720), Some(("quarter", 1)));
        assert_eq!(duration_type(1920), Some(("whole", 0)));
    }

    fn sample() -> Score {
        let mut s = Score::new("Mi obra & más", TimeSig { num: 4, den: 4 }, 2, 4);
        s.composer = "Yo".into();
        s.tempo = 90;
        s.add_part("Violonchelo", Clef::Bass, 42, false);
        s.put_note(0, 0, 0, 0, 480, vec![p(2, 5), p(4, 5)]).unwrap();
        s.put_note(0, 0, 0, 480, 960, vec![p(0, 5)]).unwrap();
        s.put_note(0, 0, 0, 1440, 960, vec![p(1, 5)]).unwrap(); // tied across the barline
        s.put_note(0, 1, 1, 0, 960, vec![p(0, 4)]).unwrap(); // second voice
        s.put_note(0, 0, 2, 0, 480, vec![p(2, 5)]).unwrap();
        s.make_tuplet(0, 0, 2, 0, 3, 2).unwrap();
        s.put_note(0, 0, 2, 160, 240, vec![p(3, 5)]).unwrap();
        s.put_note(0, 0, 2, 320, 240, vec![p(4, 5)]).unwrap();
        s.put_note(1, 0, 0, 0, 1920, vec![p(0, 3)]).unwrap();
        s.set_time_at(3, TimeSig { num: 3, den: 4 });
        s.put_note(0, 0, 3, 0, 1440, vec![p(5, 5)]).unwrap();
        s
    }

    #[test]
    fn writes_valid_xml_that_our_reader_understands() {
        let s = sample();
        let xml = export_mscx(&s);
        assert!(xml.contains("<museScore version=\"4.60\">") && xml.contains("Mi obra &amp; más"));
        assert!(xml.contains("<Spanner type=\"Tie\">") && xml.contains("<endTuplet/>"));
        let r = import_mscx(&xml).unwrap();
        let b = &r.score;
        assert_eq!((b.title.as_str(), b.composer.as_str(), b.fifths, b.tempo), ("Mi obra & más", "Yo", 2, 90));
        assert_eq!(b.time_changes, s.time_changes);
        assert_eq!(b.parts.len(), 2);
        assert_eq!((b.parts[1].name.as_str(), b.parts[1].clef, b.parts[1].program), ("Violonchelo", Clef::Bass, 42));
        assert_eq!(crate::io::sounds(b), crate::io::sounds(&s));
        // structure: same events (ignoring the origin bookkeeping)
        for (pa, pb) in s.parts.iter().zip(&b.parts) {
            assert_eq!(pa.measures, pb.measures);
        }
    }

    #[test]
    fn chord_symbols_round_trip_through_musescore() {
        for fifths in [-2i8, 0, 3] {
            let mut s = Score::new("Cifrado", TimeSig { num: 4, den: 4 }, fifths, 2);
            for m in 0..2 {
                for k in 0..4 {
                    s.put_note(0, 0, m, k * 480, 480, vec![p(0, 5)]).unwrap();
                }
            }
            s.set_chord(0, 0, "C").unwrap();
            s.set_chord(0, 960, "Bb/D").unwrap();
            s.set_chord(1, 0, "F#m7").unwrap();
            s.set_chord(1, 1440, "Edim7").unwrap();
            let xml = export_mscx(&s);
            assert_eq!(xml.matches("<Harmony>").count(), 4);
            let r = import_mscx(&xml).unwrap();
            assert_eq!(r.score.chords, s.chords, "tonalidad {fifths}");
            assert!(!r.warnings.iter().any(|w| w.contains("cifrado")), "{:?}", r.warnings);
        }
    }

    #[test]
    fn unsupported_harmony_is_kept_literally_not_imported() {
        let mut s = Score::new("x", TimeSig { num: 4, den: 4 }, 0, 1);
        s.put_note(0, 0, 0, 0, 1920, vec![p(0, 5)]).unwrap();
        s.set_chord(0, 0, "G7").unwrap();
        let xml = export_mscx(&s).replace("<extension>64</extension>", "<extension>128</extension>");
        let r = import_mscx(&xml).unwrap();
        assert!(r.score.chords.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("cifrado")));
    }

    #[test]
    fn mscz_archive_roundtrip_and_piano_staves_share_a_part() {
        let mut s = Score::new("Piano", TimeSig { num: 4, den: 4 }, 0, 1);
        s.add_part("Piano (2)", Clef::Bass, 0, false);
        s.put_note(0, 0, 0, 0, 1920, vec![p(0, 5)]).unwrap();
        let bytes = export_mscz(&s, "Piano / demo").unwrap();
        assert!(bytes.starts_with(b"PK"));
        let xml = export_mscx(&s);
        assert_eq!(xml.matches("<Part>").count(), 1); // one MuseScore part with two staves
        assert_eq!(xml.matches("<Staff id=").count(), 4);
        let r = import_mscz(&bytes).unwrap();
        assert_eq!(r.score.parts.len(), 2);
        assert_eq!(r.score.parts[1].name, "Piano (2)");
    }

    fn expressive_v4() -> String {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="4.60"><Score>
<Part><Staff id="1"><StaffType group="pitched"><name>stdNormal</name></StaffType><bracket type="1" span="1"/></Staff><trackName>Oboe</trackName>
<Instrument id="oboe"><longName>Oboe</longName><shortName>Ob.</shortName><trackName>Oboe</trackName><Channel><program value="68"/></Channel></Instrument></Part>
<Staff id="1"><Measure><startRepeat/><voice><TimeSig><sigN>4</sigN><sigD>4</sigD></TimeSig>
<Dynamic><subtype>mf</subtype></Dynamic>
<Chord><durationType>half</durationType><Spanner type="Slur"><Slur/><next><location><fractions>1/2</fractions></location></next></Spanner><Articulation><subtype>articStaccatoAbove</subtype></Articulation><Lyrics><text>la</text></Lyrics>
<Note><Fingering><text>2</text></Fingering><pitch>72</pitch><tpc>14</tpc></Note></Chord>
<Chord><durationType>half</durationType><Spanner type="Slur"><prev><location><fractions>-1/2</fractions></location></prev></Spanner><Note><pitch>74</pitch><tpc>16</tpc></Note></Chord>
<Fermata><subtype>fermataAbove</subtype></Fermata>
</voice></Measure></Staff></Score></museScore>"#
            .to_string()
    }

    #[test]
    fn local_markup_and_slurs_survive_a_round_trip() {
        let r = import_mscx(&expressive_v4()).unwrap();
        let s = &r.score;
        let ev = &s.parts[0].measures[0].events;
        assert_eq!(ev[0].dynamic, Some(Dynamic::Mf));
        assert_eq!(ev[0].articulations, vec![Artic::Staccato]);
        assert_eq!(ev[0].lyrics, vec![Lyric { verse: 0, text: "la".into(), syllabic: Syllabic::Single, extend_to: None }]);
        assert!(ev[0].extras.iter().any(|x| x.place == ExtraPlace::Note(72) && x.xml.contains("Fingering")));
        assert!(ev[1].extras.iter().all(|x| !x.xml.contains("Slur")), "slurs become real spanners, not copied markup");
        assert_eq!(s.parts[0].spanners, vec![Spanner { kind: SpannerKind::Slur, voice: 0, start: SpPos { measure: 0, tick: 0 }, end: SpPos { measure: 0, tick: 960 }, endings: 0 }]);
        assert!(ev[1].extras.iter().any(|x| x.place == ExtraPlace::After && x.xml.contains("Fermata")));
        assert!(s.parts[0].measures[0].start_repeat && s.parts[0].measures[0].extras.is_empty()); // <startRepeat/> is native now
        assert!(r.warnings.iter().any(|w| w.contains("Se conservan") && w.contains("calderones")), "{:?}", r.warnings);
        assert!(!r.warnings.iter().any(|w| w.contains("dinámicas") || w.contains("articulaciones")), "{:?}", r.warnings);
        assert!(!r.warnings.iter().any(|w| w.contains("ligaduras de expresión")), "{:?}", r.warnings);

        // written back: everything local is there, the original instrument and bracket too
        let out = export_mscx(s);
        for needle in ["<subtype>mf</subtype>", "articStaccatoAbove", "<Lyrics>", "<Fingering>", "<startRepeat/>", "<Fermata>", "<bracket type=\"1\" span=\"1\"/>", "<program value=\"68\"/>"] {
            assert!(out.contains(needle), "missing {needle}");
        }
        assert_eq!(out.matches("type=\"Slur\"").count(), 2); // start and end markers
        // and it reads back with the same markup
        let again = import_mscx(&out).unwrap().score;
        assert_eq!(again.parts[0].measures[0].events[0].extras, ev[0].extras);
        assert_eq!(again.parts[0].measures[0].events[0].dynamic, Some(Dynamic::Mf));
        assert_eq!(again.parts[0].spanners, s.parts[0].spanners);

        // editing drops what no longer applies: pitch-bound extras when transposing, chord markup on delete
        let mut e = s.clone();
        e.transpose_event(0, 0, 0, 0, 2).unwrap();
        assert!(e.parts[0].measures[0].events[0].extras.iter().all(|x| !matches!(x.place, ExtraPlace::Note(_))));
        e.delete_event(0, 0, 0, 0).unwrap();
        let ex = &e.parts[0].measures[0].events[0].extras;
        assert!(ex.iter().all(|x| matches!(x.place, ExtraPlace::Before | ExtraPlace::After)));
        // changing the instrument stops reusing the stored definition
        e.parts[0].program = 40;
        assert!(export_mscx(&e).contains("<program value=\"40\"/>") && !export_mscx(&e).contains("<program value=\"68\"/>"));
    }

    #[test]
    fn ornaments_roundtrip_through_mscx() {
        let mut s = Score::new("orn", TimeSig { num: 4, den: 4 }, 0, 1);
        for (i, a) in [Artic::Mordent, Artic::ShortTrill, Artic::Turn, Artic::InvertedTurn].into_iter().enumerate() {
            let t = i as u32 * 480;
            s.put_note(0, 0, 0, t, 480, vec![p(2, 5)]).unwrap();
            s.toggle_articulation(0, 0, SpPos { measure: 0, tick: t }, SpPos { measure: 0, tick: t }, a).unwrap();
        }
        let xml = export_mscx(&s);
        assert!(xml.contains("<subtype>ornamentMordent</subtype>") && xml.contains("<subtype>ornamentTurnInverted</subtype>"));
        let back = import_mscx(&xml).unwrap().score;
        assert_eq!(back.parts[0].measures[0].events.iter().map(|e| e.articulations.clone()).collect::<Vec<_>>(), s.parts[0].measures[0].events.iter().map(|e| e.articulations.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn septuplets_roundtrip_through_mscx() {
        let mut s = Score::new("sept", TimeSig { num: 4, den: 4 }, 0, 2);
        s.put_note(0, 0, 0, 0, 960, vec![p(0, 5)]).unwrap();
        s.make_tuplet(0, 0, 0, 0, 7, 4).unwrap();
        for i in 0..7u32 {
            let start = s.parts[0].measures[0].events[i as usize].start;
            s.put_note(0, 0, 0, start, 240, vec![p((i % 7) as u8, 5)]).unwrap();
        }
        let xml = export_mscx(&s);
        assert!(xml.contains("<normalNotes>4</normalNotes>") && xml.contains("<actualNotes>7</actualNotes>"));
        let back = import_mscx(&xml).unwrap().score;
        assert_eq!(back.parts[0].measures[0].events, s.parts[0].measures[0].events);
    }

    #[test]
    fn octave_lines_shift_written_pitch_and_pedal_is_kept() {
        let mut s = Score::new("8va", TimeSig { num: 4, den: 4 }, 0, 2);
        s.put_note(0, 0, 0, 0, 960, vec![p(0, 6)]).unwrap(); // C6 sounding
        s.put_note(0, 0, 0, 960, 960, vec![p(1, 6)]).unwrap();
        s.put_note(0, 0, 1, 0, 1920, vec![p(0, 2)]).unwrap();
        let sp = |kind, a: (usize, u32), b: (usize, u32)| Spanner { kind, voice: 0, start: SpPos { measure: a.0, tick: a.1 }, end: SpPos { measure: b.0, tick: b.1 }, endings: 0 };
        s.add_spanner(0, sp(SpannerKind::Ottava8va, (0, 0), (0, 1920))).unwrap();
        s.add_spanner(0, sp(SpannerKind::Ottava15mb, (1, 0), (1, 1920))).unwrap();
        s.add_spanner(0, sp(SpannerKind::Pedal, (0, 0), (1, 1920))).unwrap();
        let xml = export_mscx(&s);
        // written pitches: C6 (84) is written as C5 (72); C2 (36) under 15mb is written two octaves higher (60)
        assert!(xml.contains("<pitch>72</pitch>") && xml.contains("<pitch>74</pitch>") && xml.contains("<pitch>60</pitch>"));
        assert!(!xml.contains("<pitch>84</pitch>") && !xml.contains("<pitch>36</pitch>"));
        assert!(xml.contains("<subtype>8va</subtype>") && xml.contains("<subtype>15mb</subtype>") && xml.contains("type=\"Pedal\""));
        // and reading it back restores the sounding pitches and the lines
        let back = import_mscx(&xml).unwrap().score;
        assert_eq!(back.parts[0].measures, s.parts[0].measures);
        let mut a = back.parts[0].spanners.clone();
        let mut b = s.parts[0].spanners.clone();
        a.sort_by_key(|x| (x.start, x.end));
        b.sort_by_key(|x| (x.start, x.end));
        assert_eq!(a, b);
        // pedal events for playback and MIDI
        let ped = crate::io::pedals(&s);
        assert_eq!(ped.iter().map(|e| (e.at, e.down)).collect::<Vec<_>>(), vec![(0, true), (3840, false)]);
        let midi = crate::io::export_midi(&s);
        assert!(midi.windows(3).any(|w| w == [0xb0, 64, 127]) && midi.windows(3).any(|w| w == [0xb0, 64, 0]));
    }

    #[test]
    fn repeats_voltas_and_trills_roundtrip_through_mscx() {
        let mut s = Score::new("rep", TimeSig { num: 4, den: 4 }, 0, 5);
        for m in 0..5 {
            s.put_note(0, 0, m, 0, 1920, vec![p(m as u8 % 7, 5)]).unwrap();
        }
        s.toggle_repeat(0, true).unwrap();
        s.toggle_repeat(2, false).unwrap();
        let sp = |kind, a: (usize, u32), b: (usize, u32), endings| Spanner { kind, voice: 0, start: SpPos { measure: a.0, tick: a.1 }, end: SpPos { measure: b.0, tick: b.1 }, endings };
        for x in [
            sp(SpannerKind::Volta, (2, 0), (2, 1920), 0b01),
            sp(SpannerKind::Volta, (3, 0), (3, 1920), 0b11), // "1, 2"
            sp(SpannerKind::Trill, (0, 0), (1, 1920), 0),
        ] {
            s.add_spanner(0, x).unwrap();
        }
        let xml = export_mscx(&s);
        assert!(xml.contains("<startRepeat/>") && xml.contains("<endRepeat>2</endRepeat>") && xml.contains("<endings>1,2</endings>") && xml.contains("type=\"Trill\""));
        let back = import_mscx(&xml).unwrap().score;
        let mut a = back.parts[0].spanners.clone();
        let mut b = s.parts[0].spanners.clone();
        a.sort_by_key(|x| (x.start, x.end));
        b.sort_by_key(|x| (x.start, x.end));
        assert_eq!(a, b);
        assert_eq!(back.parts[0].measures.iter().map(|m| (m.start_repeat, m.end_repeat)).collect::<Vec<_>>(), s.parts[0].measures.iter().map(|m| (m.start_repeat, m.end_repeat)).collect::<Vec<_>>());
        assert_eq!(back.playback_order(), s.playback_order());
    }

    #[test]
    fn markers_and_jumps_roundtrip_through_mscx() {
        let mut s = Score::new("nav", TimeSig { num: 4, den: 4 }, 0, 6);
        s.add_part("Bajo", Clef::Bass, 0, false);
        for m in 0..6 {
            s.put_note(0, 0, m, 0, 1920, vec![p(m as u8 % 7, 5)]).unwrap();
        }
        s.toggle_marker(1, Marker::Segno).unwrap();
        s.toggle_marker(2, Marker::ToCoda).unwrap();
        s.toggle_marker(2, Marker::Fine).unwrap();
        s.toggle_marker(5, Marker::Coda).unwrap();
        s.set_jump(4, Some(Jump::DsAlCoda)).unwrap();
        let xml = export_mscx(&s);
        assert!(xml.contains("<jumpTo>segno</jumpTo>") && xml.contains("<playUntil>coda</playUntil>") && xml.contains("<continueAt>codab</continueAt>"));
        assert_eq!(xml.matches("<Marker>").count(), 4, "markers are written on the first staff only");
        let back = import_mscx(&xml).unwrap().score;
        for part in &back.parts {
            assert_eq!(part.measures.iter().map(|m| (m.markers.clone(), m.jump)).collect::<Vec<_>>(), s.parts[0].measures.iter().map(|m| (m.markers.clone(), m.jump)).collect::<Vec<_>>());
        }
        assert_eq!(back.playback_order(), s.playback_order());
        assert_eq!(&s.playback_order()[..8], &[0, 1, 2, 3, 4, 1, 2, 5]);
        // every jump form survives
        for j in [Jump::Dc, Jump::DcAlFine, Jump::DcAlCoda, Jump::Ds, Jump::DsAlFine, Jump::DsAlCoda] {
            let mut t = Score::new("j", TimeSig { num: 4, den: 4 }, 0, 2);
            t.set_jump(1, Some(j)).unwrap();
            assert_eq!(import_mscx(&export_mscx(&t)).unwrap().score.parts[0].measures[1].jump, Some(j));
        }
    }

    #[test]
    fn older_formats_are_not_copied_verbatim() {
        let v2 = expressive_v4().replace("version=\"4.60\"", "version=\"2.06\"");
        let r = import_mscx(&v2).unwrap();
        assert!(r.score.parts[0].measures[0].events.iter().all(|e| e.extras.is_empty()));
        assert!(r.score.parts[0].ms.is_none() && r.score.ms_style.is_none());
        assert!(r.warnings.iter().any(|w| w.contains("Se perderán") && w.contains("calderones")), "{:?}", r.warnings);
        // dynamics and articulations are understood in every version
        assert_eq!(r.score.parts[0].measures[0].events[0].dynamic, Some(Dynamic::Mf));
        assert_eq!(r.score.parts[0].measures[0].events[0].articulations, vec![Artic::Staccato]);
    }

    #[test]
    fn percussion_part_uses_the_drum_kit() {
        let mut s = Score::new("Drums", TimeSig { num: 4, den: 4 }, 0, 1);
        s.parts.clear();
        let i = s.add_part("Batería", Clef::Treble, 0, true);
        s.put_note(i, 0, 0, 0, 480, vec![Pitch::from_midi(38, true)]).unwrap();
        let xml = export_mscx(&s);
        assert!(xml.contains("<useDrumset>1</useDrumset>") && xml.contains("<defaultClef>PERC</defaultClef>") && xml.contains("<longName>Batería</longName>"));
        let r = import_mscx(&xml).unwrap();
        assert!(r.score.parts[0].percussion);
        assert_eq!(crate::io::sounds(&r.score)[0].midi, 38);
    }
}
