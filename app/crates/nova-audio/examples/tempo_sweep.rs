//! Estabilidad del tempo estimado frente al umbral de amplitud, sobre varias grabaciones.
//! Uso: cargo run --release --example tempo_sweep -- a.mp3 b.mp3 …   (usa target/cache/ si existe)
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};
use std::path::{Path, PathBuf};

fn load(path: &Path) -> Vec<RawNote> {
    let cache = PathBuf::from("target/cache").join(format!("{}{}.tsv", path.file_stem().unwrap().to_string_lossy(), if std::env::var("NOVA_PIANO").is_ok() { "-piano" } else { "" }));
    if let Ok(s) = std::fs::read_to_string(&cache) {
        return s.lines().filter_map(|l| {
            let v: Vec<&str> = l.split('\t').collect();
            Some(RawNote { start: v[0].parse().ok()?, end: v[1].parse().ok()?, pitch: v[2].parse().ok()?, amplitude: v[3].parse().ok()?, onset: v[4] == "1" })
        }).collect();
    }
    let notes = nova_audio::transcribe_file(path, &nova_audio::Params::default()).unwrap();
    let raw: Vec<RawNote> = notes.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset }).collect();
    std::fs::create_dir_all("target/cache").unwrap();
    let txt: String = raw.iter().map(|n| format!("{}\t{}\t{}\t{}\t{}\n", n.start, n.end, n.pitch, n.amplitude, n.onset as u8)).collect();
    std::fs::write(&cache, txt).unwrap();
    raw
}

fn main() {
    let rels = [0.3, 0.4, 0.5, 0.6, 0.65, 0.7, 0.8];
    println!("{:<44}{}", "grabación", rels.iter().map(|r| format!("{r:>8}")).collect::<String>());
    for a in std::env::args().skip(1) {
        let raw = load(Path::new(&a));
        let mut line = format!("{:<44}", Path::new(&a).file_stem().unwrap().to_string_lossy().chars().take(42).collect::<String>());
        for r in rels {
            let o = QuantizeOptions { min_relative: Some(r), robust_tempo: std::env::var("NOVA_ROBUST").is_ok(), ..Default::default() };
            line += &match notes_to_score(&raw, &o) {
                Ok(x) => format!("{:>8}", x.score.tempo),
                Err(_) => format!("{:>8}", "-"),
            };
        }
        println!("{line}");
    }
}
