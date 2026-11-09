//! Paisaje de puntuaciones de tempo: cargo run --release --example tempo_scan -- grabacion.mp3 (usa target/cache/)
use nova_core::beats::{period_scores, score_at, track_beats_refined, BeatMap};
use nova_core::quantize::detect_meter;
use nova_core::quantize::RawNote;
use std::path::Path;

fn load(path: &Path) -> Vec<RawNote> {
    let suffix = if std::env::var("NOVA_PIANO").is_ok() { "-piano" } else { "" };
    let f = format!("target/cache/{}{}.tsv", path.file_stem().unwrap().to_string_lossy(), suffix);
    std::fs::read_to_string(&f).unwrap().lines().filter_map(|l| {
        let v: Vec<&str> = l.split('\t').collect();
        Some(RawNote { start: v[0].parse().ok()?, end: v[1].parse().ok()?, pitch: v[2].parse().ok()?, amplitude: v[3].parse().ok()?, onset: v[4] == "1" })
    }).collect()
}

fn main() {
    for a in std::env::args().skip(1) {
        let raw = load(Path::new(&a));
        println!("{}", a);
        let mut amps: Vec<f32> = raw.iter().map(|n| n.amplitude).collect();
        amps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p90 = amps[amps.len() * 9 / 10];
        for rel in [0.5] {
            let w: Vec<(f64, f32)> = raw.iter().filter(|n| n.amplitude >= rel * p90).map(|n| (n.start, n.amplitude)).collect();
            let sc = period_scores(&w, 30.0);
            let prior = |p: f64| (-0.5 * ((60.0 / p / 100.0).log2() / 0.6).powi(2)).exp();
            // máximos locales
            let mut peaks: Vec<(f64, f64, f64)> = Vec::new();
            for i in 1..sc.len() - 1 {
                if sc[i].1 > sc[i - 1].1 && sc[i].1 >= sc[i + 1].1 {
                    peaks.push((60.0 / sc[i].0, sc[i].1, sc[i].1 * prior(sc[i].0)));
                }
            }
            peaks.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
            for pk in peaks.iter().take(5) {
                let p = 60.0 / pk.0;
                let beats = track_beats_refined(&w, p, 30.0);
                let map = BeatMap::from_beats(beats, p);
                let notes: Vec<RawNote> = raw.iter().filter(|n| n.amplitude >= rel * p90).copied().collect();
                let g = detect_meter(&notes, &map, None, true, false);
                println!("      {:>4.0} bpm (real {:.0}): s={:.2} s(2p)={:.2} | compás {}/4 pickup {} confianza {:.2}", pk.0, map.median_bpm(), score_at(&w, p, 30.0), score_at(&w, 2.0 * p, 30.0), g.beats, g.pickup, g.confidence);
            }
            println!("  rel {rel}: {} notas | picos (bpm: bruta → con prior): {}", w.len(), peaks.iter().take(5).map(|p| format!("{:.0}: {:.2}→{:.2}", p.0, p.1, p.2)).collect::<Vec<_>>().join(" | "));
        }
    }
}
