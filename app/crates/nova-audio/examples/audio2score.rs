//! Uso: cargo run --release --example audio2score -- entrada.wav salida.musicxml [bpm|auto] [amplitud_minima]
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("uso: audio2score entrada salida.musicxml [bpm|auto] [amplitud_minima]");
        std::process::exit(2);
    }
    let piano = std::env::var("NOVA_PIANO").ok();
    let notes = match &piano {
        Some(m) => nova_audio::piano::transcribe_file(std::path::Path::new(m), nova_audio::piano::Device::Auto, std::path::Path::new(&a[1]), &Default::default()),
        None => nova_audio::transcribe_file(std::path::Path::new(&a[1]), &nova_audio::Params::default()),
    }
    .unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });
    let raw: Vec<RawNote> = notes.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset }).collect();
    let mut opt = QuantizeOptions::default();
    opt.tempo = a.get(3).and_then(|s| s.parse().ok());
    if piano.is_some() {
        opt.min_relative = None;
        opt.min_amplitude = 0.0;
    }
    if let Some(v) = a.get(4).and_then(|s| s.parse().ok()) {
        opt.min_amplitude = v;
        opt.min_relative = None;
    }
    if let Some(v) = std::env::var("NOVA_REL").ok().and_then(|s| s.parse().ok()) {
        opt.min_relative = Some(v);
    }
    if let Some(v) = std::env::var("NOVA_VOICES").ok().and_then(|s| s.parse().ok()) {
        opt.max_voices = v;
    }
    if let Some(v) = std::env::var("NOVA_LEGATO").ok().and_then(|s| s.parse().ok()) {
        opt.legato_ticks = v;
    }
    match notes_to_score(&raw, &opt) {
        Ok(r) => {
            for w in &r.warnings {
                eprintln!("aviso: {w}");
            }
            std::fs::write(&a[2], nova_core::io::export_musicxml(&r.score)).unwrap();
            let (mut notes, mut rests) = (0, 0);
            for p in &r.score.parts {
                for m in &p.measures {
                    for e in m.events.iter().chain(m.voices.iter().flatten()) {
                        match e.content {
                            nova_core::Content::Rest => rests += 1,
                            _ => notes += 1,
                        }
                    }
                }
            }
            eprintln!("resumen: {notes} notas, {rests} silencios");
            eprintln!("{} pentagramas, {} compases, tempo {}, alteraciones {}", r.score.parts.len(), r.score.parts[0].measures.len(), r.score.tempo, r.score.fifths);
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    }
}
