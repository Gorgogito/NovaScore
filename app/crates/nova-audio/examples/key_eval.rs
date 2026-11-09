//! Evalúa variantes de la estimación de tonalidad sobre las transcripciones en caché
//! (`target/evalset/{bp,piano}/*.tsv`, generadas por `eval_scores`) transpuestas a las 12 tonalidades.
//! Uso: cargo run --release --example key_eval -- bp|piano
use nova_core::quantize::{estimate_key_with, KeyParams, RawNote};

fn real() {
    for e in std::fs::read_dir("target/cache").unwrap() {
        let p = e.unwrap().path();
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        if name.ends_with("-piano") {
            continue;
        }
        let notes: Vec<RawNote> = std::fs::read_to_string(&p)
            .unwrap()
            .lines()
            .filter_map(|l| {
                let v: Vec<&str> = l.split('\t').collect();
                Some(RawNote { start: v[0].parse().ok()?, end: v[1].parse().ok()?, pitch: v[2].parse().ok()?, amplitude: v[3].parse().ok()?, onset: v[4] == "1" })
            })
            .collect();
        let k = estimate_key_with(&notes, &KeyParams::default());
        let t = estimate_key_with(&notes, &KeyParams { temperley: true, ..Default::default() });
        println!("{:<44} Krumhansl {:>2} (corr {:.2}, margen {:.2}) | Temperley {:>2} (corr {:.2}, margen {:.2})", name.chars().take(43).collect::<String>(), k.0, k.1, k.2, t.0, t.1, t.2);
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("real") {
        return real();
    }
    let engine = std::env::args().nth(1).unwrap_or_else(|| "bp".into());
    let dir = format!("target/evalset/{engine}");
    let mut sets: Vec<(String, i32, Vec<RawNote>)> = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let txt = std::fs::read_to_string(&p).unwrap();
        let fifths: i32 = txt.lines().next().unwrap().trim_start_matches("# fifths ").parse().unwrap();
        let notes: Vec<RawNote> = txt
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let v: Vec<&str> = l.split('\t').collect();
                Some(RawNote { start: v[0].parse().ok()?, end: v[1].parse().ok()?, pitch: v[2].parse().ok()?, amplitude: v[3].parse().ok()?, onset: v[4] == "1" })
            })
            .collect();
        sets.push((p.file_stem().unwrap().to_string_lossy().chars().take(24).collect(), fifths, notes));
    }
    sets.sort_by(|a, b| a.0.cmp(&b.0));
    let norm = |f: i32| ((f % 12) + 12) % 12;
    let variants: Vec<(&str, KeyParams)> = vec![
        ("actual (Krumhansl)", KeyParams::default()),
        ("Temperley", KeyParams { temperley: true, ..Default::default() }),
        ("grave ×2", KeyParams { bass_gain: 2.0, ..Default::default() }),
        ("grave ×3", KeyParams { bass_gain: 3.0, ..Default::default() }),
        ("final ×3", KeyParams { end_gain: 3.0, ..Default::default() }),
        ("final ×6", KeyParams { end_gain: 6.0, ..Default::default() }),
        ("grave ×2 + final ×3", KeyParams { bass_gain: 2.0, end_gain: 3.0, ..Default::default() }),
        ("grave ×2 + final ×3 Temp.", KeyParams { bass_gain: 2.0, end_gain: 3.0, temperley: true }),
        ("grave ×3 + final ×6", KeyParams { bass_gain: 3.0, end_gain: 6.0, ..Default::default() }),
    ];
    print!("{:<28}", "variante");
    for s in &sets {
        print!(" {:>8}", s.0.chars().take(8).collect::<String>());
    }
    println!(" | total");
    for (name, kp) in variants {
        print!("{name:<28}");
        let (mut ok_all, mut n_all) = (0, 0);
        for (_, truth, notes) in &sets {
            let mut ok = 0;
            for s in 0..12 {
                let shifted: Vec<RawNote> = notes.iter().map(|n| RawNote { pitch: (n.pitch as i32 + s).clamp(0, 127) as u8, ..*n }).collect();
                let expect = norm(truth + 7 * s);
                if norm(estimate_key_with(&shifted, &kp).0 as i32) == expect {
                    ok += 1;
                }
            }
            print!(" {:>5}/12", ok);
            ok_all += ok;
            n_all += 12;
        }
        println!(" | {ok_all}/{n_all} = {:.0} %", ok_all as f64 * 100.0 / n_all as f64);
    }
}
