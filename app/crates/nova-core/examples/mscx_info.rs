//! Developer tool: `cargo run --release --example mscx_info -- file.mscz` prints what the
//! MuseScore importer found.
use nova_core::mscx::import_mscz;

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("cannot read file");
        let t = std::time::Instant::now();
        match import_mscz(&bytes) {
            Ok(r) => {
                let s = &r.score;
                let events: usize = s.parts.iter().flat_map(|p| p.measures.iter()).map(|m| m.events.len() + m.voices.iter().map(|v| v.len()).sum::<usize>()).sum();
                println!(
                    "{path}\n  \"{}\" by \"{}\" | {}/{} fifths {} tempo {} | {} parts, {} measures, {} events | {:?}",
                    s.title, s.composer, s.time.num, s.time.den, s.fifths, s.tempo, s.parts.len(), s.parts[0].measures.len(), events, t.elapsed()
                );
                if !s.time_changes.is_empty() {
                    println!("  time changes: {:?}", s.time_changes.iter().map(|c| (c.measure + 1, format!("{}/{}", c.time.num, c.time.den))).collect::<Vec<_>>());
                }
                for p in &s.parts {
                    println!("    - {} (program {}, vol {}{}, {:?})", p.name, p.program, p.volume, if p.percussion { ", percussion" } else { "" }, p.clef);
                }
                for w in &r.warnings {
                    println!("  ! {w}");
                }
            }
            Err(e) => println!("{path}\n  ERROR: {e}"),
        }
    }
}
