//! Viaje de ida y vuelta: partitura → audio sintético → transcripción → partitura.
//! Mide cuántas notas (altura + inicio) se recuperan.
//! Uso: cargo run --release --example roundtrip -- a.musicxml [b.musicxml …]
use nova_core::io::{import_musicxml, sounds};
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};
use std::collections::BTreeSet;

const SR: usize = 22050;

fn synth(notes: &[(f64, f64, i32)], total: f64) -> Vec<f32> {
    let mut x = vec![0.0f32; ((total + 1.0) * SR as f64) as usize];
    for &(s, d, m) in notes {
        let f = 440.0 * 2f64.powf((m as f64 - 69.0) / 12.0);
        let n = (d * SR as f64) as usize;
        let i0 = (s * SR as f64) as usize;
        for i in 0..n {
            if i0 + i >= x.len() {
                break;
            }
            let t = i as f64 / SR as f64;
            let env = (t / 0.01).min(1.0) * (-t * 1.0).exp() * ((d - t) / 0.04).clamp(0.0, 1.0);
            let w: f64 = [(1.0, 1.0), (2.0, 0.5), (3.0, 0.25), (4.0, 0.12)]
                .iter()
                .filter(|(k, _)| f * k < SR as f64 / 2.0)
                .map(|(k, a)| a * (std::f64::consts::TAU * f * k * t).sin())
                .sum();
            x[i0 + i] += (0.12 * w * env) as f32;
        }
    }
    x
}

fn main() {
    for path in std::env::args().skip(1) {
        let xml = std::fs::read_to_string(&path).unwrap();
        let src = import_musicxml(&xml).unwrap().score;
        let tempo = src.tempo as f64;
        let sec = |ticks: u32| ticks as f64 / 480.0 * 60.0 / tempo;
        let snd: Vec<_> = sounds(&src).into_iter().filter(|s| !src.parts[s.part].percussion).collect();
        if snd.len() < 4 {
            println!("{path}: demasiado corta, se omite");
            continue;
        }
        let notes: Vec<(f64, f64, i32)> = snd.iter().map(|s| (sec(s.start), sec(s.ticks), s.midi)).collect();
        let total = notes.iter().map(|n| n.0 + n.1).fold(0.0, f64::max);
        if std::env::var("NOVA_REFKEY").is_ok() {
            let rn: Vec<RawNote> = notes.iter().map(|n| RawNote { start: n.0, end: n.0 + n.1, pitch: n.2 as u8, amplitude: 0.8 }).collect();
            let mut h = [0usize; 12];
            for n in &rn { h[(n.pitch % 12) as usize] += 1; }
            println!("  clave de la fuente {} / estimada sobre notas originales {} / histograma C..B {:?}", src.fifths, nova_core::quantize::estimate_fifths(&rn), h);
        }
        let audio = synth(&notes, total);
        let t = std::time::Instant::now();
        let found = nova_audio::transcribe_samples(&audio, &nova_audio::Params::default()).unwrap();
        let raw: Vec<RawNote> = found.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude }).collect();
        let mut qo = QuantizeOptions::default();
        if let Some(v) = std::env::var("NOVA_MINAMP").ok().and_then(|s| s.parse().ok()) { qo.min_amplitude = v; }
        let r = match notes_to_score(&raw, &qo) {
            Ok(r) => r,
            Err(e) => {
                println!("{path}: ERROR {e}");
                continue;
            }
        };
        let out_tempo = r.score.tempo as f64;
        let out: Vec<_> = sounds(&r.score);
        let reference: BTreeSet<(i32, i64)> = {
            let t0 = notes.iter().map(|n| n.0).fold(f64::MAX, f64::min);
            notes.iter().map(|n| (n.2, ((n.0 - t0) * 1000.0) as i64)).collect()
        };
        let got: BTreeSet<(i32, i64)> = {
            let t0 = out.iter().map(|s| s.start).min().unwrap();
            out.iter().map(|s| (s.midi, ((s.start - t0) as f64 / 480.0 * 60.0 / out_tempo * 1000.0) as i64)).collect()
        };
        // coincidencia: misma altura y inicio dentro de 120 ms
        let tol = 120;
        let mut used = BTreeSet::new();
        let mut tp = 0;
        for &(m, t0) in &reference {
            if let Some(g) = got.iter().find(|&&(gm, gt)| gm == m && (gt - t0).abs() <= tol && !used.contains(&(gm, gt))) {
                used.insert(*g);
                tp += 1;
            }
        }
        let p = tp as f64 / got.len() as f64;
        let rc = tp as f64 / reference.len() as f64;
        println!(
            "{:<28} tempo {:>5.1}→{:>5.1}  tonal {:>2}→{:>2}  notas ref {:>3} det {:>3}  precisión {:.2} recall {:.2} F1 {:.2}  ({:.1}s)",
            std::path::Path::new(&path).file_name().unwrap().to_string_lossy(),
            tempo, out_tempo, src.fifths, r.score.fifths, reference.len(), got.len(), p, rc, 2.0 * p * rc / (p + rc).max(1e-9), t.elapsed().as_secs_f32()
        );
    }
}
