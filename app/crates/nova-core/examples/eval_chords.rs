//! Evalúa `detect_chords` sobre partituras MusicXML con cifrado (`<harmony>`), simbólicamente.
//! La verdad es un JSON por partitura (`scripts/chord_truth.py`): [{measure,tick,root,kind,bass}].
//! Uso: cargo run --release --example eval_chords -- ruta/partitura.musicxml ruta/verdad.json …
//! Mide en cada acorde de la verdad el acorde detectado vigente: fundamental, tríada (mayor/menor/dim) y calidad exacta.
use nova_core::chords::{detect_chords, Quality};

fn kind_quality(k: &str) -> Option<(Quality, u8)> {
    Some(match k {
        "major" => (Quality::Major, 0),
        "minor" => (Quality::Minor, 1),
        "dominant" => (Quality::Dominant7, 0),
        "major-seventh" => (Quality::Major7, 0),
        "minor-seventh" => (Quality::Minor7, 1),
        "diminished" => (Quality::Dim, 2),
        "diminished-seventh" => (Quality::Dim7, 2),
        _ => return None,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = std::env::var("NOVA_VERBOSE").is_ok();
    let (mut n, mut root_ok, mut triad_ok, mut exact_ok) = (0, 0, 0, 0);
    let (mut on_n, mut on_ok, mut off_n, mut off_ok) = (0, 0, 0, 0);
    for pair in args.chunks(2) {
        let score = nova_core::io::import_musicxml(&std::fs::read_to_string(&pair[0]).unwrap()).unwrap().score;
        let truth: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(&pair[1]).unwrap()).unwrap();
        let marks = detect_chords(&score);
        let (mut pn, mut pr, mut pt, mut pe) = (0, 0, 0, 0);
        for t in &truth {
            let Some((q, tri)) = kind_quality(t["kind"].as_str().unwrap_or("")) else { continue };
            let (m, tick, root) = (t["measure"].as_u64().unwrap() as usize, t["tick"].as_u64().unwrap() as u32, t["root"].as_u64().unwrap() as u8);
            let active = marks.iter().filter(|c| (c.measure, c.tick) <= (m, tick)).last();
            pn += 1;
            if tick % 480 == 0 { on_n += 1; } else { off_n += 1; }
            if let Some(c) = active {
                pr += (c.root == root) as u32;
                let tri_ok = c.quality.triad() == tri;
                pt += (c.root == root && tri_ok) as u32;
                if tick % 480 == 0 { on_ok += (c.root == root && tri_ok) as u32; } else { off_ok += (c.root == root && tri_ok) as u32; }
                pe += (c.root == root && c.quality == q) as u32;
                if verbose && !(c.root == root && tri_ok) {
                    println!("   comp {} + {}: verdad {}{} → {}", m + 1, tick, root, q.suffix(), c.name(0));
                }
            }
        }
        println!("{:<44} acordes {:>3}  fundamental {:>5.1} %  tríada {:>5.1} %  exacto {:>5.1} %", pair[0].rsplit(|c| c == '/' || c == '\\').next().unwrap(), pn, 100.0 * pr as f64 / pn.max(1) as f64, 100.0 * pt as f64 / pn.max(1) as f64, 100.0 * pe as f64 / pn.max(1) as f64);
        n += pn;
        root_ok += pr;
        triad_ok += pt;
        exact_ok += pe;
    }
    println!("en pulso: {on_ok}/{on_n} tríada correcta; fuera de pulso: {off_ok}/{off_n}");
    println!("TOTAL {n} acordes: fundamental {:.1} %, tríada {:.1} %, exacto {:.1} %", 100.0 * root_ok as f64 / n.max(1) as f64, 100.0 * triad_ok as f64 / n.max(1) as f64, 100.0 * exact_ok as f64 / n.max(1) as f64);
}
