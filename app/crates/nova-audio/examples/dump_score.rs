//! Uso: cargo run --example dump_score -- archivo.musicxml  (resumen textual de la partitura)
use nova_core::Content;
fn main() {
    let p = std::env::args().nth(1).expect("falta archivo");
    let s = nova_core::io::import_musicxml(&std::fs::read_to_string(p).unwrap()).unwrap().score;
    println!("tempo {} compas {}/{} fifths {}", s.tempo, s.time.num, s.time.den, s.fifths);
    for (pi, part) in s.parts.iter().enumerate() {
        println!("== parte {pi} {} {:?}", part.name, part.clef);
        for (mi, m) in part.measures.iter().enumerate() {
            let fmt = |evs: &Vec<nova_core::Event>| -> String {
                evs.iter().map(|e| match &e.content {
                    Content::Rest => format!("r{}", e.ticks),
                    Content::Note { pitches, tie } => format!("[{}]{}{}", pitches.iter().map(|p| p.midi().to_string()).collect::<Vec<_>>().join(","), e.ticks, if *tie { "~" } else { "" }),
                }).collect::<Vec<_>>().join(" ")
            };
            print!("c{:<2} v1: {}", mi + 1, fmt(&m.events));
            for (vi, v) in m.voices.iter().enumerate() { print!("  | v{}: {}", vi + 2, fmt(v)); }
            println!();
        }
    }
}
