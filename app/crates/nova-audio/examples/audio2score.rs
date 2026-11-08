//! Uso: cargo run --release --example audio2score -- entrada.wav salida.musicxml [bpm|auto] [amplitud_minima]
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("uso: audio2score entrada salida.musicxml [bpm|auto] [amplitud_minima]");
        std::process::exit(2);
    }
    let notes = nova_audio::transcribe_file(std::path::Path::new(&a[1]), &nova_audio::Params::default()).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });
    let raw: Vec<RawNote> = notes.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude }).collect();
    let mut opt = QuantizeOptions::default();
    opt.tempo = a.get(3).and_then(|s| s.parse().ok());
    if let Some(v) = a.get(4).and_then(|s| s.parse().ok()) {
        opt.min_amplitude = v;
    }
    match notes_to_score(&raw, &opt) {
        Ok(r) => {
            for w in &r.warnings {
                eprintln!("aviso: {w}");
            }
            std::fs::write(&a[2], nova_core::io::export_musicxml(&r.score)).unwrap();
            eprintln!("{} pentagramas, {} compases, tempo {}, alteraciones {}", r.score.parts.len(), r.score.parts[0].measures.len(), r.score.tempo, r.score.fifths);
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    }
}
