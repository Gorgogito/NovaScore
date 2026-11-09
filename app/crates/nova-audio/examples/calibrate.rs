//! Calibración de umbrales contra partituras de referencia.
//!
//! Uso: cargo run --release --example calibrate -- audio.mp3=partitura.mscz [otro.mp3=otra.mscz …]
//!
//! La grabación y la partitura no suelen coincidir en tempo ni en arreglo, así que se alinean con DTW
//! sobre cromogramas y se comparan notas (altura exacta y clase de altura) en el tiempo alineado.
//! Es una medida aproximada: sirve para comparar ajustes entre sí, no como verdad absoluta.
//! Variables: NOVA_TOL=0.3 (tolerancia en segundos de referencia).
use nova_audio::Params;
use nova_core::io::sounds;
use nova_core::mscx::import_mscz;
use nova_core::quantize::{clean_notes_with, estimate_key, CleanParams, RawNote};
use std::path::{Path, PathBuf};

const HOP: f64 = 0.2;

struct Piece {
    name: String,
    raw: Vec<RawNote>,
    /// (inicio s, fin s, altura) de referencia
    refn: Vec<(f64, f64, u8)>,
    ref_fifths: i8,
    /// frames de audio → segundo de referencia
    map: Vec<f64>,
    quality: f64,
}

fn transcribe_cached(path: &Path) -> Vec<RawNote> {
    let cache = PathBuf::from("target/cache").join(format!("{}.tsv", path.file_stem().unwrap().to_string_lossy()));
    if let Ok(s) = std::fs::read_to_string(&cache) {
        return s
            .lines()
            .filter_map(|l| {
                let v: Vec<&str> = l.split('\t').collect();
                Some(RawNote { start: v[0].parse().ok()?, end: v[1].parse().ok()?, pitch: v[2].parse().ok()?, amplitude: v[3].parse().ok()?, onset: v[4] == "1" })
            })
            .collect();
    }
    let notes = nova_audio::transcribe_file(path, &Params::default()).expect("transcripción");
    let raw: Vec<RawNote> = notes.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset }).collect();
    std::fs::create_dir_all("target/cache").unwrap();
    let txt: String = raw.iter().map(|n| format!("{}\t{}\t{}\t{}\t{}\n", n.start, n.end, n.pitch, n.amplitude, n.onset as u8)).collect();
    std::fs::write(&cache, txt).unwrap();
    raw
}

fn chroma(notes: &[(f64, f64, u8, f64)], frames: usize) -> Vec<[f64; 12]> {
    let mut c = vec![[0.0f64; 12]; frames];
    for &(s, e, p, w) in notes {
        let a = (s / HOP) as usize;
        let b = ((e / HOP) as usize).min(frames.saturating_sub(1));
        for f in a..=b.max(a) {
            if f < frames {
                c[f][(p % 12) as usize] += w;
            }
        }
    }
    for f in c.iter_mut() {
        for v in f.iter_mut() {
            *v = (1.0 + 10.0 * *v).ln();
        }
        let n = f.iter().map(|v| v * v).sum::<f64>().sqrt();
        if n > 0.0 {
            f.iter_mut().for_each(|v| *v /= n);
        }
    }
    c
}

/// DTW completo; devuelve para cada frame de `a` el frame medio de `b` y la similitud media del camino.
fn dtw(a: &[[f64; 12]], b: &[[f64; 12]]) -> (Vec<f64>, f64) {
    let (n, m) = (a.len(), b.len());
    let cost = |i: usize, j: usize| -> f32 { 1.0 - a[i].iter().zip(&b[j]).map(|(x, y)| x * y).sum::<f64>() as f32 };
    let mut d = vec![f32::INFINITY; n * m];
    let mut from = vec![0u8; n * m];
    for i in 0..n {
        for j in 0..m {
            let c = cost(i, j);
            if i == 0 && j == 0 {
                d[0] = c;
                continue;
            }
            let mut best = f32::INFINITY;
            let mut k = 0u8;
            if i > 0 && j > 0 && d[(i - 1) * m + j - 1] < best {
                best = d[(i - 1) * m + j - 1];
                k = 1;
            }
            // pasos ponderados: avanzar en solo una secuencia cuesta un poco más
            if i > 0 && d[(i - 1) * m + j] + 0.1 < best {
                best = d[(i - 1) * m + j] + 0.1;
                k = 2;
            }
            if j > 0 && d[i * m + j - 1] + 0.1 < best {
                best = d[i * m + j - 1] + 0.1;
                k = 3;
            }
            d[i * m + j] = best + c;
            from[i * m + j] = k;
        }
    }
    let (mut i, mut j) = (n - 1, m - 1);
    let mut sums = vec![(0.0f64, 0usize); n];
    let mut sim = 0.0;
    let mut cnt = 0;
    loop {
        sums[i].0 += j as f64;
        sums[i].1 += 1;
        sim += 1.0 - cost(i, j) as f64;
        cnt += 1;
        if i == 0 && j == 0 {
            break;
        }
        match from[i * m + j] {
            1 => {
                i -= 1;
                j -= 1;
            }
            2 => i -= 1,
            _ => j -= 1,
        }
    }
    let mut map: Vec<f64> = sums.iter().map(|&(s, c)| if c > 0 { s / c as f64 } else { f64::NAN }).collect();
    for k in 1..n {
        if map[k].is_nan() {
            map[k] = map[k - 1];
        }
    }
    (map, sim / cnt as f64)
}

fn load(audio: &str, score: &str) -> Piece {
    let raw = transcribe_cached(Path::new(audio));
    let r = import_mscz(&std::fs::read(score).expect("partitura")).expect("mscz");
    let sc = &r.score;
    let spt = 60.0 / sc.tempo as f64 / 480.0;
    let refn: Vec<(f64, f64, u8)> = sounds(sc)
        .into_iter()
        .filter(|s| !sc.parts[s.part].percussion)
        .map(|s| (s.start as f64 * spt, (s.start + s.ticks) as f64 * spt, s.midi.clamp(0, 127) as u8))
        .collect();
    let total_a = raw.iter().map(|n| n.end).fold(0.0, f64::max);
    let total_r = refn.iter().map(|n| n.1).fold(0.0, f64::max);
    let fa = (total_a / HOP) as usize + 1;
    let fr = (total_r / HOP) as usize + 1;
    let pa = {
        let mut amps: Vec<f32> = raw.iter().map(|n| n.amplitude).collect();
        amps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p90 = amps[amps.len() * 9 / 10];
        raw.iter().filter(|n| n.amplitude >= 0.65 * p90).map(|n| (n.start, n.end, n.pitch, n.amplitude as f64)).collect::<Vec<_>>()
    };
    let pr: Vec<_> = refn.iter().map(|&(s, e, p)| (s, e, p, 1.0)).collect();
    let (map_f, quality) = dtw(&chroma(&pa, fa), &chroma(&pr, fr));
    Piece {
        name: Path::new(audio).file_stem().unwrap().to_string_lossy().into_owned(),
        raw,
        refn,
        ref_fifths: sc.fifths,
        map: map_f.iter().map(|f| f * HOP).collect(),
        quality,
    }
}

fn to_ref_time(p: &Piece, t: f64) -> f64 {
    let x = t / HOP;
    let i = (x as usize).min(p.map.len() - 1);
    let j = (i + 1).min(p.map.len() - 1);
    let f = x - i as f64;
    p.map[i] * (1.0 - f) + p.map[j] * f.clamp(0.0, 1.0)
}

/// (precisión, recall, F1) con altura exacta; y F1 con clase de altura.
fn score(p: &Piece, notes: &[RawNote], tol: f64) -> (f64, f64, f64, f64) {
    let mapped: Vec<(f64, u8)> = notes.iter().map(|n| (to_ref_time(p, n.start), n.pitch)).collect();
    let eval = |same: &dyn Fn(u8, u8) -> bool| -> (f64, f64) {
        let hit_n = mapped.iter().filter(|&&(t, pi)| p.refn.iter().any(|&(rs, re, rp)| same(pi, rp) && t >= rs - tol && t <= re + tol * 0.5)).count();
        let hit_r = p.refn.iter().filter(|&&(rs, re, rp)| mapped.iter().any(|&(t, pi)| same(pi, rp) && t >= rs - tol && t <= re + tol * 0.5)).count();
        (hit_n as f64 / mapped.len().max(1) as f64, hit_r as f64 / p.refn.len().max(1) as f64)
    };
    let (pr, rc) = eval(&|a, b| a == b);
    let (pc, rcc) = eval(&|a, b| a % 12 == b % 12);
    let f = |p: f64, r: f64| if p + r > 0.0 { 2.0 * p * r / (p + r) } else { 0.0 };
    (pr, rc, f(pr, rc), f(pc, rcc))
}

fn run(p: &Piece, rel: Option<f64>, cp: Option<&CleanParams>, tol: f64) -> (usize, (f64, f64, f64, f64)) {
    let mut amps: Vec<f32> = p.raw.iter().map(|n| n.amplitude).collect();
    amps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p90 = amps[amps.len() * 9 / 10];
    let thr = rel.map_or(0.0, |r| (r * p90 as f64).max(0.2) as f32);
    let notes: Vec<RawNote> = p.raw.iter().copied().filter(|n| n.amplitude >= thr && (21..=108).contains(&n.pitch)).collect();
    let notes = match cp {
        Some(c) => clean_notes_with(&notes, c).0,
        None => notes,
    };
    (notes.len(), score(p, &notes, tol))
}

fn main() {
    let tol: f64 = std::env::var("NOVA_TOL").ok().and_then(|s| s.parse().ok()).unwrap_or(0.3);
    let pieces: Vec<Piece> = std::env::args()
        .skip(1)
        .map(|a| {
            let (au, sc) = a.split_once('=').expect("formato audio=partitura");
            let p = load(au, sc);
            let (fifths, corr, margin) = estimate_key(&p.raw);
            println!("{}: {} notas detectadas, {} de referencia, calidad de alineación {:.3}, tonalidad ref {} / audio {} (corr {:.2}, margen {:.2})", p.name, p.raw.len(), p.refn.len(), p.quality, p.ref_fifths, fifths, corr, margin);
            p
        })
        .collect();
    let base = CleanParams::default();
    let report = |label: &str, rel: Option<f64>, cp: Option<CleanParams>| {
        let mut line = format!("{label:<34}");
        let (mut sf, mut sc) = (0.0, 0.0);
        for p in &pieces {
            let (n, (pr, rc, f, fc)) = run(p, rel, cp.as_ref(), tol);
            line += &format!(" | {:>5} P{:.2} R{:.2} F{:.2}", n, pr, rc, f);
            sf += f;
            sc += fc;
        }
        println!("{line} | media F {:.3} (clase {:.3})", sf / pieces.len() as f64, sc / pieces.len() as f64);
    };
    println!("\n== sin limpieza, umbral relativo");
    for r in [0.0, 0.3, 0.4, 0.5, 0.65, 0.8, 1.0] {
        report(&format!("rel {r:.2} sin limpiar"), if r == 0.0 { None } else { Some(r) }, None);
    }
    println!("\n== con limpieza por defecto, umbral relativo");
    for r in [0.3, 0.4, 0.5, 0.65, 0.8, 1.0] {
        report(&format!("rel {r:.2} limpio"), Some(r), Some(base));
    }
    println!("\n== margen de tonalidad (rel 0.65)");
    for m in [0.0, 0.05, 0.08, 0.12, 0.2, 0.3, 9.0] {
        report(&format!("margen {m}"), Some(0.65), Some(CleanParams { key_min_margin: m, ..base }));
    }
    println!("\n== razón de armónicos (rel 0.65)");
    for h in [0.0f32, 0.4, 0.6, 0.8, 1.0, 1.5] {
        report(&format!("armónico ≤ {h}"), Some(0.65), Some(CleanParams { harmonic_ratio: h, ..base }));
    }
    println!("\n== percentil cromático (rel 0.65)");
    for c in [0usize, 30, 60, 80, 95] {
        report(&format!("cromático pct {c}"), Some(0.65), Some(CleanParams { chromatic_pct: c, ..base }));
    }
}
