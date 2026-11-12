//! Developer tool: how faithfully does `import_midi` rebuild a score? MuseScore exports a MusicXML
//! sample to MIDI, we import that MIDI, and the sounding notes are compared with the original ones.
//!
//! `cargo run --release --example midi_check -- samples/tuplets.musicxml out.mid`
use nova_core::io::{import_midi, import_musicxml, sounds};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, out) = (&args[1], &args[2]);
    let a = import_musicxml(&std::fs::read_to_string(input).expect("input")).expect("musicxml").score;
    let ms = std::env::var("MUSESCORE").unwrap_or_else(|_| r"C:\Program Files\MuseScore 4\bin\MuseScore4.exe".into());
    std::process::Command::new(&ms).args(["-o", out, input]).status().expect("MuseScore");
    let r = import_midi(&std::fs::read(out).expect("midi")).expect("midi import");
    let flat = |s: &nova_core::Score| {
        let mut v: Vec<(u32, u32, i32)> = sounds(s).iter().map(|x| (x.start, x.ticks, x.midi)).collect();
        v.sort();
        v
    };
    let (fa, fb) = (flat(&a), flat(&r.score));
    let hit = fa.iter().filter(|x| fb.contains(x)).count();
    let tup: usize = r.score.parts.iter().flat_map(|p| p.measures.iter()).flat_map(|m| m.events.iter().chain(m.voices.iter().flatten())).filter(|e| e.tuplet.is_some()).count();
    let voices: usize = r.score.parts.iter().map(|p| p.measures.iter().map(|m| m.voices.iter().filter(|v| !v.is_empty()).count()).max().unwrap_or(0)).sum();
    println!("original {} notas ({} partes) / importado {} notas ({} partes, {} eventos en tuplet, {} voces extra)", fa.len(), a.parts.len(), fb.len(), r.score.parts.len(), tup, voices);
    println!("coinciden exactamente (inicio, duración, altura): {hit} de {} ({:.0} %)", fa.len(), 100.0 * hit as f64 / fa.len().max(1) as f64);
    for w in &r.warnings {
        println!("  ! {w}");
    }
}
