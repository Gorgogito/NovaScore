//! MuseScore reader: `.mscz` (zip) and `.mscx` (XML), formats 2.x, 3.x and 4.x.
//!
//! What is imported: title and composer, instruments (one part per staff), clefs, key and
//! time signature, tempo, notes, chords, rests, ties, up to four voices per staff and tuplets.
//! Everything else (dynamics, slurs, articulations, lyrics, repeats...) is reported in the
//! import warnings instead of being silently dropped.

use crate::io::{build_measures, ImportReport, Raw};
use crate::*;
use roxmltree::{Document, Node};
use std::collections::{BTreeSet, HashMap};
use std::io::Read;

/// Import a `.mscz` archive or a plain `.mscx` file from its bytes.
pub fn import_mscz(bytes: &[u8]) -> Result<ImportReport, String> {
    if !bytes.starts_with(b"PK") {
        let text = std::str::from_utf8(bytes).map_err(|_| "El archivo no es un .mscz ni un .mscx válido".to_string())?;
        return import_mscx(text);
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("Archivo .mscz inválido: {e}"))?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|c| c.into_owned())).collect();

    // The root score is named in META-INF/container.xml; otherwise take the first .mscx.
    let mut root: Option<String> = None;
    if let Ok(mut f) = zip.by_name("META-INF/container.xml") {
        let mut s = String::new();
        if f.read_to_string(&mut s).is_ok() {
            if let Ok(doc) = Document::parse(&s) {
                // MuseScore 4 lists several files (style, thumbnail...); the score is the .mscx one.
                root = doc
                    .descendants()
                    .filter(|n| n.has_tag_name("rootfile"))
                    .filter_map(|n| n.attribute("full-path"))
                    .find(|p| p.to_lowercase().ends_with(".mscx"))
                    .map(str::to_string);
            }
        }
    }
    let name = root
        .filter(|r| names.contains(r))
        .or_else(|| names.iter().find(|n| n.to_lowercase().ends_with(".mscx") && !n.contains('/')).cloned())
        .or_else(|| names.iter().find(|n| n.to_lowercase().ends_with(".mscx")).cloned())
        .ok_or("El .mscz no contiene ninguna partitura (.mscx)")?;
    let mut text = String::new();
    zip.by_name(&name)
        .map_err(|e| e.to_string())?
        .read_to_string(&mut text)
        .map_err(|e| format!("No se pudo leer la partitura del .mscz: {e}"))?;
    let style = names.iter().find(|n| n.to_lowercase().ends_with(".mss")).cloned().and_then(|n| {
        let mut s = String::new();
        zip.by_name(&n).ok()?.read_to_string(&mut s).ok()?;
        Some(s)
    });
    import_mscx_styled(&text, style)
}

// ------------------------------------------------------------------ helpers

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.has_tag_name(name))
}

fn text_of(n: Node, name: &str) -> Option<String> {
    child(n, name).map(|c| c.text().unwrap_or("").trim().to_string())
}

/// A MuseScore `<Marker>`: segno, coda (label `codab`), "To Coda" (label `coda`) or fine. Others are not understood.
fn marker_of(n: Node) -> Option<Marker> {
    if !n.has_tag_name("Marker") {
        return None;
    }
    let label = text_of(n, "label").filter(|l| !l.is_empty()).or_else(|| text_of(n, "subtype"))?;
    match label.as_str() {
        "segno" => Some(Marker::Segno),
        "codab" => Some(Marker::Coda),
        "coda" => Some(Marker::ToCoda),
        "fine" => Some(Marker::Fine),
        _ => None,
    }
}

/// A MuseScore `<Jump>`, told apart by where it goes (`jumpTo`) and where it stops (`playUntil`).
fn jump_of(n: Node) -> Option<Jump> {
    if !n.has_tag_name("Jump") {
        return None;
    }
    let to = text_of(n, "jumpTo")?;
    let until = text_of(n, "playUntil").unwrap_or_default();
    let cont = text_of(n, "continueAt").unwrap_or_default();
    // "play repeats" after the jump is not supported: the jump would sound different.
    if text_of(n, "playRepeats").map_or(false, |v| v.trim() == "1") {
        return None;
    }
    match (to.as_str(), until.as_str(), cont.as_str()) {
        ("start", "end", "") => Some(Jump::Dc),
        ("start", "fine", "") => Some(Jump::DcAlFine),
        ("start", "coda", "codab") => Some(Jump::DcAlCoda),
        ("segno", "end", "") => Some(Jump::Ds),
        ("segno", "fine", "") => Some(Jump::DsAlFine),
        ("segno", "coda", "codab") => Some(Jump::DsAlCoda),
        _ => None,
    }
}

fn int_of(n: Node, name: &str) -> Option<i64> {
    text_of(n, name).and_then(|t| t.parse::<f64>().ok()).map(|f| f.round() as i64)
}

/// "3/8" -> ticks (whole note = 1920).
fn frac_ticks(s: &str) -> Option<i64> {
    let (a, b) = s.trim().split_once('/')?;
    let (a, b): (i64, i64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    if b == 0 {
        return None;
    }
    Some(a * 1920 / b)
}

/// Whole-note length of a `<duration>` element in either notation (`4/4` or `z="4" n="4"`).
fn duration_ticks(n: Node) -> Option<i64> {
    let d = child(n, "duration")?;
    if let (Some(z), Some(nn)) = (d.attribute("z"), d.attribute("n")) {
        return Some(z.parse::<i64>().ok()? * 1920 / nn.parse::<i64>().ok()?.max(1));
    }
    frac_ticks(d.text()?)
}

fn base_ticks(duration_type: &str) -> Option<i64> {
    Some(match duration_type {
        "long" => 7680,
        "breve" => 3840,
        "whole" => 1920,
        "half" => 960,
        "quarter" => 480,
        "eighth" => 240,
        "16th" => 120,
        "32nd" => 60,
        "64th" => 30,
        "128th" => 15,
        "256th" => 8,
        _ => return None,
    })
}

/// Pitch spelling from MuseScore's "tonal pitch class" (14 = C, +1 per fifth up).
fn spell(midi: i32, tpc: Option<i32>) -> Pitch {
    if let Some(t) = tpc.filter(|t| (-1..=33).contains(t)) {
        const STEPS: [u8; 7] = [3, 0, 4, 1, 5, 2, 6]; // F C G D A E B
        const NATURAL: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
        let f = t - 14;
        let step = STEPS[(f + 1).rem_euclid(7) as usize];
        let alter = (f + 1).div_euclid(7);
        let o = midi - alter - NATURAL[step as usize];
        if o.rem_euclid(12) == 0 && alter.abs() <= 2 {
            return Pitch { step, alter: alter as i8, octave: (o.div_euclid(12) - 1) as i8 };
        }
    }
    Pitch::from_midi(midi, true)
}

/// (measures, ticks) of a MuseScore 4 `<location>`, relative to the element it belongs to.
fn location_of(n: Node) -> (i32, i64) {
    let loc = child(n, "location");
    let measures = loc.and_then(|l| int_of(l, "measures")).unwrap_or(0) as i32;
    let ticks = loc.and_then(|l| text_of(l, "fractions")).and_then(|s| frac_ticks(&s)).unwrap_or(0);
    (measures, ticks)
}

/// Dynamic marking of a MuseScore `<Dynamic>` element, when it is one NovaScore knows.
fn dynamic_of(n: Node) -> Option<Dynamic> {
    text_of(n, "subtype").and_then(|s| Dynamic::from_name(&s))
}

/// Articulation of a MuseScore `<Articulation>` element ("articStaccatoAbove", "staccato"...).
fn artic_of(n: Node) -> Option<Artic> {
    let s = text_of(n, "subtype")?.to_lowercase();
    let s = s.strip_prefix("artic").unwrap_or(&s);
    let s = s.strip_suffix("above").or_else(|| s.strip_suffix("below")).unwrap_or(s);
    match s {
        "staccato" => Some(Artic::Staccato),
        "staccatissimo" => Some(Artic::Staccatissimo),
        "tenuto" => Some(Artic::Tenuto),
        "accent" | "sforzato" => Some(Artic::Accent),
        "marcato" => Some(Artic::Marcato),
        "tenutostaccato" | "portato" => Some(Artic::Portato),
        "ornamentmordent" => Some(Artic::Mordent),
        "ornamentshorttrill" | "ornamentprall" => Some(Artic::ShortTrill),
        "ornamentturn" => Some(Artic::Turn),
        "ornamentturninverted" => Some(Artic::InvertedTurn),
        _ => None,
    }
}

/// Bit mask of the endings of a volta ("1", "1,2" -> bits 0 and 1).
fn endings_mask(s: Option<String>) -> u8 {
    s.unwrap_or_default().split(',').filter_map(|n| n.trim().parse::<u32>().ok()).filter(|n| (1..=8).contains(n)).fold(0u8, |m, n| m | 1 << (n - 1))
}

/// Octave line from `<subtype>`: "8va", "8vb", "15ma", "15mb" (older files use 0-3).
fn ottava_kind(subtype: Option<String>) -> SpannerKind {
    match subtype.as_deref().map(str::trim) {
        Some("8vb") | Some("1") => SpannerKind::Ottava8vb,
        Some("15ma") | Some("2") => SpannerKind::Ottava15ma,
        Some("15mb") | Some("3") => SpannerKind::Ottava15mb,
        _ => SpannerKind::Ottava8va,
    }
}

fn hairpin_kind(subtype: Option<i64>) -> SpannerKind {
    // 0 and 2 grow (hairpin / "cresc." line), 1 and 3 fade
    if matches!(subtype, Some(1) | Some(3)) { SpannerKind::Decrescendo } else { SpannerKind::Crescendo }
}

fn clef_of(s: &str) -> (Clef, bool) {
    let s = s.trim();
    if s.starts_with("PERC") {
        (Clef::Treble, true)
    } else if s.starts_with('F') {
        (Clef::Bass, false)
    } else {
        (Clef::Treble, false)
    }
}

fn key_fifths(n: Node) -> Option<i8> {
    int_of(n, "concertKey").or_else(|| int_of(n, "accidental")).or_else(|| int_of(n, "key")).map(|v| v.clamp(-7, 7) as i8)
}

fn plain_text(n: Node) -> String {
    n.descendants().filter(|d| d.is_text()).map(|d| d.text().unwrap_or("")).collect::<String>().trim().to_string()
}

struct StaffInfo {
    id: String,
    name: String,
    program: u8,
    volume: u8,
    clef: Clef,
    percussion: bool,
    clef_set: bool,
    group: u32,
    instrument_xml: Option<String>,
    staff_xml: Option<String>,
}

/// Elements that are read but not represented yet, with the label used in the warning.
/// Returns (kept, lost): features that are not shown by NovaScore but are copied back when the
/// score is saved as MuseScore, and features that would be dropped on saving.
fn ignored_features(score: Node, preserve: bool, first_staff: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut kept: BTreeSet<&'static str> = BTreeSet::new();
    let mut lost: BTreeSet<&'static str> = BTreeSet::new();
    for n in score.descendants().filter(|n| n.is_element()) {
        if n.has_tag_name("Harmony") && is_native_harmony(n, first_staff) {
            continue;
        }
        // Dynamics and articulations NovaScore understands are imported, not ignored; the
        // <Articulation> entries of instrument definitions have no subtype.
        if marker_of(n).is_some() || jump_of(n).is_some() {
            continue;
        }
        if (n.has_tag_name("Dynamic") && dynamic_of(n).is_some()) || (n.has_tag_name("Articulation") && (text_of(n, "subtype").is_none() || artic_of(n).is_some())) || (n.has_tag_name("Ornament") && artic_of(n).is_some()) {
            continue;
        }
        let label = match n.tag_name().name() {
            "Dynamic" => "dinámicas",
            "Spanner" => match n.attribute("type") {
                _ => continue,
            },
            "Articulation" | "Artic" => "otras articulaciones",
            "Lyrics" => "letra",
            "StaffText" | "SystemText" | "TempoText" => "textos de pentagrama",
            "Harmony" => "cifrado de acordes",
            "Fermata" => "calderones",
            "Arpeggio" => "arpegios",
            "Ornament" => "ornamentos",
            "Fingering" => "digitaciones",
            "Jump" | "Marker" => "saltos y marcas poco habituales (no se ejecutan)",
            "Image" => "imágenes",
            _ => continue,
        };
        // Spanners (slurs, hairpins...) point at other notes, so they cannot be copied safely.
        let spanner = matches!(n.tag_name().name(), "Slur" | "HairPin" | "Ottava" | "Pedal" | "Trill" | "Volta" | "Spanner");
        if preserve && !spanner {
            kept.insert(label);
        } else {
            lost.insert(label);
        }
    }
    (kept.into_iter().collect(), lost.into_iter().collect())
}

// -------------------------------------------------------------------- import

pub fn import_mscx(text: &str) -> Result<ImportReport, String> {
    import_mscx_styled(text, None)
}

/// Elements that stay local to a place in the score and can be copied back verbatim.
/// El acorde de un `<Harmony>` de MuseScore: (fundamental, calidad, bajo). `None` si no es un cifrado
/// que NovaScore entienda (sin raíz, o con una calidad fuera de su modelo).
fn harmony_of(el: Node) -> Option<(u8, crate::chords::Quality, u8)> {
    let root = int_of(el, "root")? as i32;
    // MuseScore guarda la calidad como identificador de su lista estándar (`<extension>`, lo habitual en
    // los archivos reales) o como texto (`<name>`); sin ninguno, el acorde es mayor.
    let quality = match (text_of(el, "name"), int_of(el, "extension")) {
        (Some(name), _) => crate::chords::ChordMark::parse(&format!("C{name}"))?.1,
        (None, Some(id)) => crate::chords::quality_of_mscx_chord_id(id)?,
        (None, None) => crate::chords::Quality::Major,
    };
    let pc = crate::chords::tpc_to_pc(root);
    let bass = int_of(el, "base").map_or(pc, |b| crate::chords::tpc_to_pc(b as i32));
    Some((pc, quality, bass))
}

/// ¿Es un cifrado que NovaScore importa de forma nativa? Solo el de la primera voz del primer pentagrama.
fn is_native_harmony(n: Node, first_staff: &str) -> bool {
    if harmony_of(n).is_none() {
        return false;
    }
    let Some(parent) = n.parent_element() else { return false };
    let Some(measure) = (if parent.has_tag_name("voice") { parent.parent_element() } else { Some(parent) }) else { return false };
    let first_voice = !parent.has_tag_name("voice") || measure.children().find(|c| c.has_tag_name("voice")) == Some(parent);
    first_voice && measure.parent_element().and_then(|s| s.attribute("id")) == Some(first_staff)
}

const VOICE_SKIP: [&str; 10] = ["Chord", "Rest", "Tuplet", "endTuplet", "location", "TimeSig", "KeySig", "Clef", "Tempo", "Spanner"];
const CHORD_SKIP: [&str; 6] = ["durationType", "dots", "Note", "Tuplet", "Spanner", "duration"];
const NOTE_SKIP: [&str; 6] = ["pitch", "tpc", "tpc2", "Tie", "Spanner", "play"];

pub fn import_mscx_styled(text: &str, style: Option<String>) -> Result<ImportReport, String> {
    let doc = Document::parse(text).map_err(|e| format!("XML de MuseScore inválido: {e}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("museScore") {
        return Err("No es un archivo de MuseScore".into());
    }
    let version = root.attribute("version").unwrap_or("?").to_string();
    // Verbatim copies are only valid inside a MuseScore 4 file.
    let preserve = version.starts_with('4');
    let src = |n: Node| text[n.range()].to_string();
    let score_el = child(root, "Score").ok_or("El archivo no contiene una partitura")?;

    let mut warnings: Vec<String> = Vec::new();
    let mut score = Score::new("Sin título", TimeSig { num: 4, den: 4 }, 0, 0);
    score.parts.clear();
    if preserve {
        score.ms_style = style;
    }

    // ---- metadata
    let meta = |name: &str| {
        score_el
            .children()
            .find(|c| c.has_tag_name("metaTag") && c.attribute("name") == Some(name))
            .map(|c| c.text().unwrap_or("").trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let styled_text = |style: &str| {
        score_el
            .descendants()
            .filter(|n| n.has_tag_name("Text"))
            .find(|n| text_of(*n, "style").as_deref() == Some(style))
            .and_then(|n| child(n, "text").map(plain_text))
            .filter(|s| !s.is_empty())
    };
    if let Some(t) = meta("workTitle").or_else(|| styled_text("Title")) {
        score.title = t;
    }
    if let Some(c) = meta("composer").or_else(|| styled_text("Composer")) {
        score.composer = c.lines().next().unwrap_or("").trim().to_string();
    }

    // ---- parts and staves (one of our parts per staff)
    let mut staves: Vec<StaffInfo> = Vec::new();
    let mut transposing = false;
    for (group, part) in score_el.children().filter(|c| c.has_tag_name("Part")).enumerate() {
        let inst = child(part, "Instrument");
        let mut name = inst.and_then(|i| text_of(i, "longName")).filter(|s| !s.is_empty()).or_else(|| text_of(part, "trackName")).filter(|s| !s.is_empty()).unwrap_or_else(|| "Instrumento".into());
        if let Some(n) = inst.and_then(|i| text_of(i, "trackName")).filter(|s| !s.is_empty() && name == "Instrumento") {
            name = n;
        }
        let channel = inst.and_then(|i| {
            let chans: Vec<Node> = i.children().filter(|c| c.has_tag_name("Channel")).collect();
            chans.iter().find(|c| c.attribute("name").is_none()).or(chans.first()).copied()
        });
        let program = channel.and_then(|c| child(c, "program")).and_then(|p| p.attribute("value")).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).clamp(0, 127) as u8;
        let volume = channel
            .and_then(|c| c.children().find(|x| x.has_tag_name("controller") && x.attribute("ctrl") == Some("7")))
            .and_then(|x| x.attribute("value"))
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(100)
            .clamp(0, 127) as u8;
        if let Some(i) = inst {
            if int_of(i, "transposeChromatic").unwrap_or(0) != 0 || int_of(i, "transposeDiatonic").unwrap_or(0) != 0 {
                transposing = true;
            }
        }
        let part_staves: Vec<Node> = part.children().filter(|c| c.has_tag_name("Staff")).collect();
        for (k, st) in part_staves.iter().enumerate() {
            let percussion = child(*st, "StaffType").and_then(|t| t.attribute("group")) == Some("percussion");
            let clef_txt = text_of(*st, "defaultClef").or_else(|| text_of(*st, "defaultConcertClef")).or_else(|| text_of(*st, "defaultTransposingClef"));
            let (clef, perc_clef) = clef_txt.as_deref().map(clef_of).unwrap_or((Clef::Treble, false));
            staves.push(StaffInfo {
                // MuseScore 4.6 omits the id inside <Part>: staves are then numbered in order.
                id: st.attribute("id").map(str::to_string).unwrap_or_else(|| (staves.len() + 1).to_string()),
                name: if k > 0 { format!("{name} ({})", k + 1) } else { name.clone() },
                program,
                volume,
                clef,
                percussion: percussion || perc_clef,
                clef_set: clef_txt.is_some(),
                group: group as u32,
                instrument_xml: if preserve { inst.map(src) } else { None },
                staff_xml: if preserve {
                    // contents between <Staff ...> and </Staff>
                    let full = src(*st);
                    full.find('>').and_then(|a| full.rfind("</Staff>").map(|b| full[a + 1..b].to_string()))
                } else {
                    None
                },
            });
        }
    }
    if staves.is_empty() {
        return Err("La partitura no define ningún instrumento".into());
    }

    // ---- measures of every staff
    let mut time_map: std::collections::BTreeMap<usize, TimeSig> = Default::default();
    let mut got_key = false;
    let mut got_tempo = false;
    let (mut key_changes, mut clef_changes, mut irregular, mut dropped_voices) = (0usize, 0usize, 0usize, 0usize);
    let mut max_measures = 0usize;
    let mut harmony_chords: Vec<crate::chords::ChordMark> = Vec::new();
    let mut raw_staves: Vec<Vec<Vec<Vec<Raw>>>> = (0..staves.len()).map(|_| Vec::new()).collect(); // staff -> voice -> measure -> events
    let mut measure_extras: Vec<Vec<Vec<String>>> = (0..staves.len()).map(|_| Vec::new()).collect(); // staff -> measure -> fragments
    let mut measure_nav: Vec<Vec<(Vec<Marker>, Option<Jump>)>> = (0..staves.len()).map(|_| Vec::new()).collect();
    let mut measure_repeats: Vec<Vec<(bool, u8)>> = (0..staves.len()).map(|_| Vec::new()).collect();
    let mut staff_spanners: Vec<Vec<(SpannerKind, usize, SpPos, SpPos, u8)>> = (0..staves.len()).map(|_| Vec::new()).collect();

    for st in score_el.children().filter(|c| c.has_tag_name("Staff")) {
        let Some(si) = staves.iter().position(|s| Some(s.id.as_str()) == st.attribute("id")) else { continue };
        let mut cur_len: i64 = score.time.measure_ticks() as i64;
        // MuseScore 2/3 tuplet definitions are referenced by id from the chords.
        let mut tuplet_defs: HashMap<String, (u8, u8)> = HashMap::new();
        let mut tuplet_starts: HashMap<(usize, String), i64> = HashMap::new();
        // Slurs and hairpins: start positions waiting for their end (MuseScore 2/3 ids), and
        // ones that give their end relative to the start (MuseScore 4 locations).
        let mut meas_lens: Vec<i64> = Vec::new();
        // (kind, voice, start, ..., endings of a volta)
        let mut sp_open: HashMap<String, (SpannerKind, usize, (usize, i64), u8)> = HashMap::new();
        let mut sp_abs: Vec<(SpannerKind, usize, (usize, i64), (usize, i64), u8)> = Vec::new();
        let mut sp_rel: Vec<(SpannerKind, usize, (usize, i64), i32, i64, u8)> = Vec::new();
        let mut pickup_shift: i64 = 0;
        let voices = &mut raw_staves[si];

        for (mi, me) in st.children().filter(|c| c.has_tag_name("Measure")).enumerate() {
            max_measures = max_measures.max(mi + 1);

            // Signatures and tempo, anywhere inside the measure.
            for el in me.descendants().filter(|n| n.is_element()) {
                match el.tag_name().name() {
                    "TimeSig" => {
                        if let (Some(n), Some(d)) = (int_of(el, "sigN"), int_of(el, "sigD")) {
                            if n > 0 && n <= 32 && matches!(d, 1 | 2 | 4 | 8 | 16) {
                                let ts = TimeSig { num: n as u32, den: d as u32 };
                                time_map.entry(mi).or_insert(ts);
                                if mi == 0 {
                                    score.time = ts;
                                }
                                cur_len = ts.measure_ticks() as i64;
                            }
                        }
                    }
                    "KeySig" => {
                        if let Some(f) = key_fifths(el) {
                            if !got_key {
                                score.fifths = f;
                                got_key = true;
                            } else if f != score.fifths {
                                key_changes += 1;
                            }
                        }
                    }
                    "Clef" => {
                        let txt = text_of(el, "concertClefType").or_else(|| text_of(el, "transposingClefType")).or_else(|| text_of(el, "subtype")).or_else(|| el.text().map(|t| t.trim().to_string()));
                        if let Some(t) = txt.filter(|t| !t.is_empty()) {
                            let (c, perc) = clef_of(&t);
                            if !staves[si].clef_set && mi == 0 {
                                staves[si].clef = c;
                                staves[si].percussion |= perc;
                                staves[si].clef_set = true;
                            } else if c != staves[si].clef {
                                clef_changes += 1;
                            }
                        }
                    }
                    "Tempo" if !got_tempo => {
                        if let Some(t) = child(el, "tempo").and_then(|c| c.text()).and_then(|t| t.trim().parse::<f64>().ok()) {
                            score.tempo = ((t * 60.0).round() as u32).clamp(20, 300);
                            got_tempo = true;
                        }
                    }
                    _ => {}
                }
            }

            if preserve {
                let frags: Vec<String> = me
                    .children()
                    .filter(|c| c.is_element() && !matches!(c.tag_name().name(), "voice" | "Spanner" | "startRepeat" | "endRepeat") && marker_of(*c).is_none() && jump_of(*c).is_none())
                    .map(src)
                    .collect();
                while measure_extras[si].len() <= mi {
                    measure_extras[si].push(Vec::new());
                }
                measure_extras[si][mi] = frags;
            }
            // Navigation: segno, coda, fine and D.C./D.S. are measure children
            while measure_nav[si].len() <= mi {
                measure_nav[si].push((Vec::new(), None));
            }
            for c in me.children().filter(|c| c.is_element()) {
                if let Some(mk) = marker_of(c) {
                    if !measure_nav[si][mi].0.contains(&mk) {
                        measure_nav[si][mi].0.push(mk);
                    }
                } else if let Some(j) = jump_of(c) {
                    measure_nav[si][mi].1 = Some(j);
                }
            }
            // Repeat signs
            while measure_repeats[si].len() <= mi {
                measure_repeats[si].push((false, 0));
            }
            measure_repeats[si][mi] = (
                child(me, "startRepeat").is_some(),
                child(me, "endRepeat").map_or(0, |e| e.text().and_then(|t| t.trim().parse::<u8>().ok()).unwrap_or(2).max(2)),
            );
            let measure_len = me.attribute("len").and_then(frac_ticks).unwrap_or(cur_len);
            meas_lens.push(measure_len);
            if measure_len != cur_len {
                irregular += 1;
            }

            // Voice streams: elements directly in the measure form the first voice (MuseScore 2),
            // and every <voice> element is another one (MuseScore 3/4 wrap all voices).
            let is_content = |n: &Node| n.is_element() && matches!(n.tag_name().name(), "Chord" | "Rest" | "Tuplet" | "location");
            let direct = me.children().any(|c| is_content(&c));
            let mut streams: Vec<Node> = Vec::new();
            if direct {
                streams.push(me);
            }
            streams.extend(me.children().filter(|c| c.is_element() && c.has_tag_name("voice")));

            for (vi, container) in streams.iter().enumerate() {
                if vi >= MAX_VOICES {
                    dropped_voices += 1;
                    continue;
                }
                while voices.len() <= vi {
                    voices.push(Vec::new());
                }
                while voices[vi].len() <= mi {
                    voices[vi].push(Vec::new());
                }
                let mut pos: i64 = 0;
                let mut stack: Vec<(u8, u8, i64)> = Vec::new(); // MuseScore 4: <Tuplet> ... <endTuplet/>
                let mut grp_acc: Option<(u32, f64)> = None; // written ticks so far in the current tuplet group
                let mut raws: Vec<Raw> = Vec::new();
                let mut pending: Vec<Extra> = Vec::new(); // voice-level markup waiting for the next chord
                let mut pending_dyn: Option<Dynamic> = None;

                for el in container.children().filter(|c| c.is_element()) {
                    let native_dyn = el.has_tag_name("Dynamic") && dynamic_of(el).is_some();
                    let native_harmony = si == 0 && vi == 0 && el.has_tag_name("Harmony") && harmony_of(el).is_some();
                    if native_harmony {
                        if let Some((root, quality, bass)) = harmony_of(el) {
                            let shift = if mi == 0 && measure_len < cur_len { cur_len - measure_len } else { 0 };
                            harmony_chords.push(crate::chords::ChordMark { measure: mi, tick: (pos + shift).max(0) as u32, root, quality, bass, manual: true });
                        }
                    }
                    if native_dyn {
                        pending_dyn = dynamic_of(el);
                    }
                    if preserve && !native_dyn && !native_harmony && !VOICE_SKIP.contains(&el.tag_name().name()) && !el.has_tag_name("voice") {
                        pending.push(Extra { place: ExtraPlace::Before, xml: src(el) });
                    }
                    match el.tag_name().name() {
                        "Tuplet" if child(el, "normalNotes").is_some() => {
                            let a = int_of(el, "actualNotes").unwrap_or(3).clamp(1, 255) as u8;
                            let n = int_of(el, "normalNotes").unwrap_or(2).clamp(1, 255) as u8;
                            match el.attribute("id") {
                                Some(id) => {
                                    tuplet_defs.insert(id.to_string(), (a, n));
                                }
                                None => stack.push((a, n, pos)),
                            }
                        }
                        "endTuplet" => {
                            stack.pop();
                        }
                        "Spanner" if matches!(el.attribute("type"), Some("HairPin") | Some("Ottava") | Some("Pedal") | Some("Trill") | Some("Volta")) => {
                            if child(el, "next").is_some() {
                                let (kind, endings) = match el.attribute("type") {
                                    Some("Ottava") => (ottava_kind(child(el, "Ottava").and_then(|o| text_of(o, "subtype"))), 0),
                                    Some("Pedal") => (SpannerKind::Pedal, 0),
                                    Some("Trill") => (SpannerKind::Trill, 0),
                                    Some("Volta") => (SpannerKind::Volta, endings_mask(child(el, "Volta").and_then(|v| text_of(v, "endings")))),
                                    _ => (hairpin_kind(child(el, "HairPin").and_then(|h| int_of(h, "subtype"))), 0),
                                };
                                let (m, t) = location_of(child(el, "next").unwrap());
                                sp_rel.push((kind, vi, (mi, pos), m, t, endings));
                            }
                        }
                        "HairPin" | "Ottava" | "Pedal" | "Trill" | "Volta" if el.attribute("id").is_some() => {
                            let (kind, endings) = match el.tag_name().name() {
                                "Ottava" => (ottava_kind(text_of(el, "subtype")), 0),
                                "Pedal" => (SpannerKind::Pedal, 0),
                                "Trill" => (SpannerKind::Trill, 0),
                                "Volta" => (SpannerKind::Volta, endings_mask(text_of(el, "endings"))),
                                _ => (hairpin_kind(int_of(el, "subtype")), 0),
                            };
                            sp_open.insert(format!("h{}", el.attribute("id").unwrap()), (kind, vi, (mi, pos), endings));
                        }
                        "endSpanner" => {
                            if let Some((kind, v, start, endings)) = el.attribute("id").and_then(|id| sp_open.remove(&format!("h{id}"))) {
                                sp_abs.push((kind, v, start, (mi, pos), endings));
                            }
                        }
                        "location" => {
                            if let Some(f) = text_of(el, "fractions").and_then(|s| frac_ticks(&s)) {
                                pos += f;
                            }
                        }
                        "Chord" | "Rest" => {
                            let is_chord = el.has_tag_name("Chord");
                            if is_chord && el.children().any(|c| c.is_element() && (c.tag_name().name().starts_with("grace") || matches!(c.tag_name().name(), "acciaccatura" | "appoggiatura"))) {
                                continue; // grace notes take no time
                            }
                            let dt = text_of(el, "durationType").unwrap_or_default();
                            let measure_rest = dt == "measure";
                            let written: f64 = if measure_rest {
                                duration_ticks(el).unwrap_or(measure_len) as f64
                            } else {
                                let base = base_ticks(&dt).unwrap_or(480) as f64;
                                let dots = int_of(el, "dots").unwrap_or(0).clamp(0, 4) as i32;
                                base * (2.0 - 0.5f64.powi(dots))
                            };
                            // Tuplet ratio: MuseScore 2/3 reference (<Tuplet>id</Tuplet>) or MuseScore 4 stack.
                            let v2_ref = if is_chord || el.has_tag_name("Rest") { child(el, "Tuplet").and_then(|c| c.text()).map(|t| t.trim().to_string()) } else { None };
                            let (mut a_prod, mut n_prod) = (1.0f64, 1.0f64);
                            let mut tup: Option<(u8, u8, u32)> = None;
                            if let Some(id) = v2_ref.as_ref().filter(|s| !s.is_empty()) {
                                if let Some(&(a, n)) = tuplet_defs.get(id) {
                                    a_prod = a as f64;
                                    n_prod = n as f64;
                                    let gs = *tuplet_starts.entry((mi * 8 + vi, id.clone())).or_insert(pos);
                                    tup = Some((a, n, gs.max(0) as u32));
                                }
                            } else {
                                for &(a, n, _) in &stack {
                                    a_prod *= a as f64;
                                    n_prod *= n as f64;
                                }
                                if stack.len() == 1 {
                                    tup = Some((stack[0].0, stack[0].1, stack[0].2.max(0) as u32));
                                }
                            }
                            // Inside a group positions are rounded from its start so that a 7:4 adds up exactly.
                            let real = match tup {
                                Some((a, n, gs)) if (v2_ref.is_some() || stack.len() == 1) && !measure_rest => {
                                    let acc = match grp_acc {
                                        Some((g, x)) if g == gs => x,
                                        _ => 0.0,
                                    };
                                    grp_acc = Some((gs, acc + written));
                                    let at = |x: f64| (x * n as f64 / a as f64 + 1e-9).round();
                                    (at(acc + written) - at(acc)) as i64
                                }
                                _ => {
                                    grp_acc = None;
                                    (written * n_prod / a_prod).round() as i64
                                }
                            };
                            let mut pitches = Vec::new();
                            let mut tie = false;
                            if is_chord {
                                for note in el.children().filter(|c| c.is_element() && c.has_tag_name("Note")) {
                                    let Some(midi) = int_of(note, "pitch") else { continue };
                                    let tpc = int_of(note, "tpc").map(|v| v as i32);
                                    if let Some(t2) = int_of(note, "tpc2") {
                                        if Some(t2 as i32) != tpc {
                                            transposing = true;
                                        }
                                    }
                                    pitches.push(if staves[si].percussion { Pitch::from_midi(midi as i32, true) } else { spell(midi as i32, tpc) });
                                    tie |= note.children().any(|c| {
                                        c.is_element()
                                            && (c.has_tag_name("Tie") || (c.has_tag_name("Spanner") && c.attribute("type") == Some("Tie") && child(c, "next").is_some()))
                                    });
                                }
                            }
                            let rest = !is_chord || pitches.is_empty();
                            let mut extras: Vec<Extra> = std::mem::take(&mut pending);
                            if preserve {
                                for c in el.children().filter(|c| c.is_element() && !CHORD_SKIP.contains(&c.tag_name().name()) && !((c.has_tag_name("Articulation") || c.has_tag_name("Ornament")) && artic_of(*c).is_some())) {
                                    extras.push(Extra { place: ExtraPlace::Inside, xml: src(c) });
                                }
                                for note in el.children().filter(|c| c.is_element() && c.has_tag_name("Note")) {
                                    let Some(midi) = int_of(note, "pitch") else { continue };
                                    for c in note.children().filter(|c| c.is_element() && !NOTE_SKIP.contains(&c.tag_name().name())) {
                                        extras.push(Extra { place: ExtraPlace::Note(midi as i32), xml: src(c) });
                                    }
                                }
                            }
                            if is_chord {
                                for c in el.children().filter(|c| c.is_element()) {
                                    match c.tag_name().name() {
                                        // MuseScore 4: <Spanner type="Slur"><next><location>..
                                        "Spanner" if c.attribute("type") == Some("Slur") => {
                                            if let Some(nx) = child(c, "next") {
                                                let (m, t) = location_of(nx);
                                                sp_rel.push((SpannerKind::Slur, vi, (mi, pos), m, t, 0));
                                            }
                                        }
                                        // MuseScore 2/3: <Slur type="start|stop" id="..."/>
                                        "Slur" if c.attribute("type").is_some() => {
                                            let key = format!("s{}", c.attribute("id").unwrap_or(""));
                                            match c.attribute("type") {
                                                Some("start") => {
                                                    sp_open.insert(key, (SpannerKind::Slur, vi, (mi, pos), 0));
                                                }
                                                Some("stop") => {
                                                    if let Some((kind, v, start, _)) = sp_open.remove(&key) {
                                                        sp_abs.push((kind, v, start, (mi, pos), 0));
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            if !measure_rest || real > 0 {
                                let mut articulations: Vec<Artic> = Vec::new();
                                for c in el.children().filter(|c| c.is_element() && (c.has_tag_name("Articulation") || c.has_tag_name("Ornament"))) {
                                    if let Some(a) = artic_of(c) {
                                        if !articulations.contains(&a) {
                                            articulations.push(a);
                                        }
                                    }
                                }
                                raws.push(Raw { start: pos.max(0) as u32, ticks: real.max(0) as u32, pitches, rest, tie, tup, extras, dynamic: pending_dyn.take(), articulations });
                            }
                            pos += real;
                        }
                        _ => {}
                    }
                }

                // Markup after the last chord of the voice.
                if let Some(last) = raws.last_mut() {
                    last.extras.extend(pending.drain(..).map(|e| Extra { place: ExtraPlace::After, xml: e.xml }));
                }

                // A short first measure is a pick-up bar: align its end with the barline.
                if mi == 0 && measure_len < cur_len {
                    let shift = (cur_len - measure_len) as u32;
                    pickup_shift = shift as i64;
                    for r in &mut raws {
                        r.start += shift;
                        if let Some(t) = r.tup.as_mut() {
                            t.2 += shift;
                        }
                    }
                }
                voices[vi][mi] = raws;
            }
        }
        // Resolve relative ends (MuseScore 4) into absolute positions, then shift the pick-up bar.
        let len_at = |m: i64| -> i64 { meas_lens.get(m.max(0) as usize).copied().or(meas_lens.last().copied()).unwrap_or(1920) };
        for (kind, vi, start, measures, ticks, endings) in sp_rel {
            let (mut m, mut t) = (start.0 as i64 + measures as i64, start.1 + ticks);
            while t > len_at(m) {
                t -= len_at(m);
                m += 1;
            }
            while t < 0 && m > 0 {
                m -= 1;
                t += len_at(m);
            }
            sp_abs.push((kind, vi, start, (m.max(0) as usize, t.max(0)), endings));
        }
        for (kind, vi, a, b, endings) in sp_abs {
            let fix = |p: (usize, i64)| -> SpPos {
                let mut tick = p.1;
                if p.0 == 0 {
                    tick += pickup_shift;
                }
                SpPos { measure: p.0, tick: tick.max(0) as u32 }
            };
            staff_spanners[si].push((kind, vi, fix(a), fix(b), endings));
        }
        // MuseScore stores the written pitch of notes under an octave line; the sounding pitch
        // (what NovaScore keeps) is the written one moved by the line.
        for &(kind, _, a, b, _) in &staff_spanners[si] {
            let k = kind.octaves();
            if k == 0 {
                continue;
            }
            for voice in voices.iter_mut() {
                for (mi, raws) in voice.iter_mut().enumerate() {
                    for r in raws.iter_mut() {
                        let pos = SpPos { measure: mi, tick: r.start };
                        if pos >= a && pos < b {
                            for p in r.pitches.iter_mut() {
                                p.octave += k as i8;
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- build our model
    score.time = time_map.get(&0).copied().unwrap_or(score.time);
    let mut prev = score.time;
    for (&mi, &ts) in &time_map {
        if mi > 0 && ts != prev {
            score.time_changes.push(TimeChange { measure: mi, time: ts });
            prev = ts;
        }
    }
    let lens: Vec<u32> = (0..max_measures.max(1)).map(|i| score.measure_len(i)).collect();
    for (si, info) in staves.iter().enumerate() {
        let mut raw = std::mem::take(&mut raw_staves[si]);
        if raw.is_empty() {
            raw.push(Vec::new());
        }
        let mut measures = build_measures(&raw, max_measures.max(1), &lens);
        for (mi, frags) in measure_extras[si].iter().enumerate() {
            if let Some(me) = measures.get_mut(mi) {
                me.extras = frags.clone();
            }
        }
        for (mi, &(start, end)) in measure_repeats[si].iter().enumerate() {
            if let Some(me) = measures.get_mut(mi) {
                me.start_repeat = start;
                me.end_repeat = end;
            }
        }
        // Markers and jumps are written on the first staff only but apply to every part.
        for (mi, (marks, jump)) in measure_nav[0].iter().enumerate() {
            if let Some(me) = measures.get_mut(mi) {
                me.markers = marks.clone();
                me.jump = *jump;
            }
        }
        let mut part = Part::new(info.name.clone(), info.clef, info.program, measures);
        part.spanners = staff_spanners[si]
            .iter()
            .filter_map(|&(kind, vi, start, mut end, endings)| {
                // A hairpin that stops on a barline belongs to the measure it closes.
                if kind != SpannerKind::Slur && end.tick == 0 && end.measure > 0 {
                    end = SpPos { measure: end.measure - 1, tick: lens[end.measure - 1] };
                }
                let ok = vi < MAX_VOICES && start < end && end.measure < part.measures.len() && end.tick <= lens.get(end.measure).copied().unwrap_or(0);
                ok.then_some(Spanner { kind, voice: vi as u8, start, end, endings })
            })
            .collect();
        part.volume = info.volume;
        part.percussion = info.percussion;
        if let (Some(ix), Some(sx)) = (&info.instrument_xml, &info.staff_xml) {
            part.ms = Some(MsOrigin {
                group: info.group,
                instrument_xml: ix.clone(),
                staff_xml: sx.clone(),
                name: info.name.clone(),
                program: info.program,
                clef: info.clef,
                percussion: info.percussion,
            });
        }
        score.parts.push(part);
    }

    // ---- warnings
    if key_changes > 0 {
        warnings.push(format!("Hay {key_changes} cambio(s) de armadura: solo se usa la inicial."));
    }
    if clef_changes > 0 {
        warnings.push(format!("Hay {clef_changes} cambio(s) de clave que no se importaron."));
    }
    if irregular > 0 {
        warnings.push(format!("{irregular} compás(es) de duración irregular (anacrusis o compases cortados) se ajustaron al compás completo."));
    }
    if dropped_voices > 0 {
        warnings.push("Se descartaron voces adicionales (máximo 4 por pentagrama).".into());
    }
    if transposing {
        warnings.push("Hay instrumentos transpositores: las notas se muestran en altura real (de concierto).".into());
    }
    harmony_chords.sort_by_key(|c| (c.measure, c.tick));
    harmony_chords.dedup_by_key(|c| (c.measure, c.tick));
    score.chords = harmony_chords;
    let (kept, lost) = ignored_features(score_el, preserve, &staves[0].id);
    if !kept.is_empty() {
        warnings.push(format!("Se conservan al guardar como MuseScore, aunque no se muestran aquí: {}.", kept.join(", ")));
    }
    if !lost.is_empty() {
        warnings.push(format!("Se perderán al guardar como MuseScore: {}.", lost.join(", ")));
    }
    warnings.insert(0, format!("Archivo de MuseScore versión {version}."));
    Ok(ImportReport { score, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tpc_spelling() {
        let p = |midi, tpc| spell(midi, Some(tpc));
        assert_eq!(p(60, 14), Pitch { step: 0, alter: 0, octave: 4 }); // C4
        assert_eq!(p(78, 20), Pitch { step: 3, alter: 1, octave: 5 }); // F#5
        assert_eq!(p(70, 12), Pitch { step: 6, alter: -1, octave: 4 }); // Bb4
        assert_eq!(p(59, 7), Pitch { step: 0, alter: -1, octave: 4 }); // Cb4 sounds B3
        assert_eq!(p(61, 21), Pitch { step: 0, alter: 1, octave: 4 }); // C#4
        assert_eq!(spell(61, None), Pitch::from_midi(61, true));
    }

    fn mscx4(measures: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="4.20"><Score>
<metaTag name="workTitle">Prueba</metaTag><metaTag name="composer">Yo</metaTag>
<Part><Staff id="1"><StaffType group="pitched"/></Staff><trackName>Violín</trackName>
<Instrument id="violin"><longName>Violín</longName><Channel><program value="40"/><controller ctrl="7" value="90"/></Channel></Instrument></Part>
<Staff id="1">{measures}</Staff></Score></museScore>"#
        )
    }

    #[test]
    fn reads_v4_notes_ties_voices_and_tuplets() {
        let m1 = r#"<Measure><voice>
<KeySig><concertKey>2</concertKey></KeySig><TimeSig><sigN>3</sigN><sigD>4</sigD></TimeSig>
<Tempo><tempo>1.5</tempo></Tempo>
<Clef><concertClefType>G</concertClefType></Clef>
<Chord><durationType>quarter</durationType><Note><Spanner type="Tie"><Tie/><next><location><fractions>1/4</fractions></location></next></Spanner><pitch>66</pitch><tpc>20</tpc></Note></Chord>
<Chord><durationType>quarter</durationType><Note><Spanner type="Tie"><prev><location><fractions>-1/4</fractions></location></prev></Spanner><pitch>66</pitch><tpc>20</tpc></Note></Chord>
<Chord><durationType>quarter</durationType><Note><pitch>71</pitch><tpc>19</tpc></Note><Note><pitch>62</pitch><tpc>16</tpc></Note></Chord>
</voice><voice><Rest><durationType>half</durationType></Rest><Chord><durationType>quarter</durationType><Note><pitch>55</pitch><tpc>15</tpc></Note></Chord></voice></Measure>"#;
        let m2 = r#"<Measure><voice>
<Tuplet><normalNotes>2</normalNotes><actualNotes>3</actualNotes><baseNote>eighth</baseNote></Tuplet>
<Chord><durationType>eighth</durationType><Note><pitch>67</pitch><tpc>15</tpc></Note></Chord>
<Chord><durationType>eighth</durationType><Note><pitch>69</pitch><tpc>17</tpc></Note></Chord>
<Chord><durationType>eighth</durationType><Note><pitch>71</pitch><tpc>19</tpc></Note></Chord>
<endTuplet/>
<Rest><durationType>half</durationType></Rest>
<Rest><durationType>quarter</durationType></Rest>
</voice></Measure>"#;
        let r = import_mscx(&mscx4(&format!("{m1}{m2}"))).unwrap();
        let s = &r.score;
        assert_eq!((s.title.as_str(), s.composer.as_str()), ("Prueba", "Yo"));
        assert_eq!((s.time, s.fifths, s.tempo), (TimeSig { num: 3, den: 4 }, 2, 90));
        let part = &s.parts[0];
        assert_eq!((part.name.as_str(), part.program, part.volume), ("Violín", 40, 90));
        assert_eq!(part.measures.len(), 2);
        // measure 1: tied F#4 pair, quarter chord B4 + D4
        let v1 = &part.measures[0].events;
        assert_eq!(v1[0].content, Content::note(vec![Pitch { step: 3, alter: 1, octave: 4 }], true));
        assert_eq!(v1[1].content, Content::note(vec![Pitch { step: 3, alter: 1, octave: 4 }], false));
        assert_eq!(v1[2].ticks, 480);
        assert!(matches!(&v1[2].content, Content::Note { pitches, .. } if pitches.len() == 2));
        // second voice: half rest then G3
        let v2 = part.measures[0].voice(1).unwrap();
        assert_eq!((v2[0].content.clone(), v2[0].ticks), (Content::Rest, 960));
        assert!(matches!(v2[1].content, Content::Note { .. }));
        // measure 2: eighth triplet
        let ev = &part.measures[1].events;
        assert!(ev[..3].iter().all(|e| e.ticks == 160 && e.notated_ticks() == 240 && e.tuplet.is_some()));
        // sounding: tied notes merge
        let snd = crate::io::sounds(s);
        assert!(snd.iter().any(|x| x.midi == 66 && x.ticks == 960));
    }

    #[test]
    fn reads_v2_layout_with_tuplet_ids_and_pickup() {
        let xml = r#"<?xml version="1.0"?><museScore version="2.06"><Score>
<Part><Staff id="1"><StaffType group="pitched"/></Staff><Staff id="2"><defaultClef>F</defaultClef></Staff><trackName>Piano</trackName>
<Instrument><longName>Piano</longName><Channel><program value="0"/></Channel></Instrument></Part>
<Staff id="1">
<Measure number="1" len="1/4"><TimeSig><sigN>4</sigN><sigD>4</sigD></TimeSig>
<Chord><durationType>quarter</durationType><Note><pitch>60</pitch><tpc>14</tpc></Note></Chord></Measure>
<Measure number="2">
<Tuplet id="1"><normalNotes>2</normalNotes><actualNotes>3</actualNotes><baseNote>eighth</baseNote></Tuplet>
<Chord><Tuplet>1</Tuplet><durationType>eighth</durationType><Note><Tie id="3"/><pitch>62</pitch><tpc>16</tpc></Note></Chord>
<Chord><Tuplet>1</Tuplet><durationType>eighth</durationType><Note><pitch>64</pitch><tpc>18</tpc></Note></Chord>
<Chord><Tuplet>1</Tuplet><durationType>eighth</durationType><Note><pitch>65</pitch><tpc>13</tpc></Note></Chord>
<Rest><durationType>half</durationType></Rest><Rest><durationType>quarter</durationType></Rest></Measure>
</Staff>
<Staff id="2"><Measure number="1" len="1/4"><Rest><durationType>quarter</durationType></Rest></Measure>
<Measure number="2"><Rest><durationType>measure</durationType><duration z="4" n="4"/></Rest></Measure></Staff>
</Score></museScore>"#;
        let r = import_mscx(xml).unwrap();
        let s = &r.score;
        assert_eq!(s.parts.len(), 2); // grand staff -> two staves
        assert_eq!(s.parts[1].clef, Clef::Bass);
        assert_eq!(s.parts[1].name, "Piano (2)");
        // pick-up: the note sits on the last beat of the first measure
        let m0 = &s.parts[0].measures[0].events;
        assert_eq!(m0.last().unwrap().start, 1440);
        assert!(matches!(m0.last().unwrap().content, Content::Note { .. }));
        let m1 = &s.parts[0].measures[1].events;
        assert!(m1[..3].iter().all(|e| e.tuplet.map(|t| t.start) == Some(0) && e.ticks == 160));
        assert!(matches!(m1[0].content, Content::Note { tie: true, .. }));
        assert!(r.warnings.iter().any(|w| w.contains("irregular")));
    }

    #[test]
    fn reads_mscz_archive_with_several_rootfiles() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("META-INF/container.xml", o).unwrap();
            z.write_all(br#"<container><rootfiles><rootfile full-path="score_style.mss"/><rootfile full-path="Mi obra.mscx"/><rootfile full-path="Thumbnails/thumbnail.png"/></rootfiles></container>"#).unwrap();
            z.start_file("score_style.mss", o).unwrap();
            z.write_all(b"<museScore version=\"4.00\"><Style/></museScore>").unwrap();
            z.start_file("Mi obra.mscx", o).unwrap();
            z.write_all(mscx4(r#"<Measure><voice><Chord><durationType>whole</durationType><Note><pitch>60</pitch><tpc>14</tpc></Note></Chord></voice></Measure>"#).as_bytes()).unwrap();
            z.finish().unwrap();
        }
        let r = import_mscz(&buf).unwrap();
        assert_eq!(r.score.title, "Prueba");
        // a bare .mscx (not zipped) works too, and v4 `defaultConcertClef` is honoured
        let bare = mscx4("<Measure><voice><Rest><durationType>measure</durationType><duration>4/4</duration></Rest></voice></Measure>").replace("<StaffType group=\"pitched\"/>", "<StaffType group=\"pitched\"/><defaultConcertClef>F8vb</defaultConcertClef>");
        assert_eq!(import_mscz(bare.as_bytes()).unwrap().score.parts[0].clef, Clef::Bass);
    }

    #[test]
    fn reads_time_signature_changes() {
        let m = |ts: &str, dur: &str| format!("<Measure><voice>{ts}<Rest><durationType>measure</durationType><duration>{dur}</duration></Rest></voice></Measure>");
        let sig = |n, d| format!("<TimeSig><sigN>{n}</sigN><sigD>{d}</sigD></TimeSig>");
        let xml = mscx4(&format!("{}{}{}{}", m(&sig(4, 4), "4/4"), m("", "4/4"), m(&sig(3, 4), "3/4"), m(&sig(3, 4), "3/4")));
        let r = import_mscx(&xml).unwrap();
        let s = &r.score;
        assert_eq!(s.time, TimeSig { num: 4, den: 4 });
        assert_eq!(s.time_changes, vec![TimeChange { measure: 2, time: TimeSig { num: 3, den: 4 } }]); // repeat at 3 dropped
        assert_eq!(s.measure_len(3), 1440);
        assert!(s.parts[0].measures.iter().enumerate().all(|(i, me)| me.events.iter().map(|e| e.ticks).sum::<u32>() == s.measure_len(i)));
        assert!(!r.warnings.iter().any(|w| w.contains("compás")), "{:?}", r.warnings);
    }

    #[test]
    fn warns_about_unsupported_content_and_rejects_garbage() {
        let m = r#"<Measure><voice><Dynamic><subtype>p</subtype></Dynamic><Chord><durationType>whole</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure>"#;
        let r = import_mscx(&mscx4(m)).unwrap();
        let w = r.warnings.join(" ");
        assert!(w.contains("letra") && !w.contains("dinámicas"), "{w}");
        assert_eq!(r.score.parts[0].measures[0].events[0].dynamic, Some(Dynamic::P));
        assert!(import_mscx("hola").is_err());
        assert!(import_mscx("<otra/>").is_err());
        assert!(import_mscz(b"PK nope").is_err());
    }
}
