//! Viaje de ida y vuelta: partitura → audio sintético (con rubato opcional) → transcripción →
//! partitura. Mide cuántas notas se recuperan en su posición rítmica exacta (±1/2 semicorchea),
//! con tempo constante y siguiendo el tempo.
//!
//! Uso: cargo run --release --example roundtrip -- a.musicxml [b.musicxml …]
//! Variables: NOVA_RUBATO=0.12 (amplitud del rubato, 0 = sin rubato), NOVA_MINAMP=0.5 (umbral absoluto; sin él se usa el relativo NOVA_REL=0.65),
//!            NOVA_TIGHT=30 (rigidez del seguimiento), NOVA_PERIOD=16 (pulsos por ciclo de rubato)
use nova_core::io::{import_musicxml, sounds};
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};
use std::collections::BTreeSet;

const SR: usize = 22050;

fn env_f(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

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

/// (precisión, recall, F1) de posiciones (altura, tick relativo a la primera nota), ±`tol` ticks.
fn f1(reference: &BTreeSet<(i32, i64)>, got: &BTreeSet<(i32, i64)>, tol: i64) -> (f64, f64, f64) {
    let mut used = BTreeSet::new();
    let mut tp = 0;
    for &(m, t0) in reference {
        if let Some(g) = got.iter().find(|&&(gm, gt)| gm == m && (gt - t0).abs() <= tol && !used.contains(&(gm, gt))) {
            used.insert(*g);
            tp += 1;
        }
    }
    let p = tp as f64 / got.len().max(1) as f64;
    let r = tp as f64 / reference.len().max(1) as f64;
    (p, r, 2.0 * p * r / (p + r).max(1e-9))
}

/// Pieza sintética con ritmo variado: bajo con notas largas en los tiempos fuertes y melodía con
/// patrones rítmicos que se repiten (hay acentos, a diferencia de un chorro uniforme).
fn synthetic(bars: u32) -> nova_core::Score {
    use nova_core::{Clef, Pitch, Score, TimeSig};
    let mut s = Score::new("sintetica", TimeSig { num: 4, den: 4 }, 0, 1);
    let lh = s.add_part("Bajo", Clef::Bass, 0, false);
    let p = |m: i32| Pitch::from_midi(m, true);
    // duraciones de la melodía por compás (en ticks; suman 1920); 0 = silencio
    let patterns: [&[(u32, bool)]; 5] = [
        &[(480, true), (240, true), (240, true), (480, true), (480, true)],
        &[(720, true), (240, true), (960, true)],
        &[(240, true), (240, true), (480, true), (480, false), (480, true)],
        &[(480, true), (480, true), (240, true), (240, true), (240, true), (240, true)],
        &[(960, true), (480, true), (240, true), (240, true)],
    ];
    let scale = [60, 62, 64, 67, 69, 72, 74, 76];
    let mut seed = 12345u32;
    let mut rnd = |n: u32| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 16) % n
    };
    let mut idx = 3usize;
    for bar in 0..bars {
        let base = bar * 1920;
        // bajo: blanca en el 1, negras en el 3 y 4
        let root = [48, 43, 45, 41][(bar % 4) as usize];
        s.put_abs(lh, base, 960, &[p(root)]).unwrap();
        s.put_abs(lh, base + 960, 480, &[p(root + 7)]).unwrap();
        s.put_abs(lh, base + 1440, 480, &[p(root + 12)]).unwrap();
        let pat = patterns[rnd(5) as usize];
        let mut t = base;
        for &(d, sounding) in pat {
            if sounding {
                let step = rnd(3) as i32 - 1;
                idx = (idx as i32 + step).clamp(0, scale.len() as i32 - 1) as usize;
                s.put_abs(0, t, d, &[p(scale[idx])]).unwrap();
            }
            t += d;
        }
    }
    s
}

/// Pieza sintética con tresillos: melodía de negras y pulsos de tres corcheas de tresillo (o
/// negra + corchea de tresillo) sobre un bajo en negras.
fn synthetic_triplets(bars: u32) -> nova_core::Score {
    use nova_core::{Clef, Pitch, Score, TimeSig};
    let mut s = Score::new("tresillos", TimeSig { num: 4, den: 4 }, 0, 1);
    let lh = s.add_part("Bajo", Clef::Bass, 0, false);
    let p = |m: i32| Pitch::from_midi(m, true);
    let scale = [60, 62, 64, 67, 69, 72, 74, 76];
    let mut seed = 777u32;
    let mut rnd = |n: u32| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 16) % n
    };
    let mut idx = 3i32;
    let step = |idx: &mut i32, rnd: &mut dyn FnMut(u32) -> u32| {
        *idx = (*idx + rnd(3) as i32 - 1).clamp(0, 7);
        scale[*idx as usize]
    };
    for bar in 0..bars {
        let base = bar * 1920;
        let root = [48, 43, 45, 41][(bar % 4) as usize];
        for q in 0..4 {
            s.put_abs(lh, base + q * 480, 480, &[p(root + [0, 7, 12, 7][q as usize])]).unwrap();
        }
        for beat in 0..4u32 {
            let at = base + beat * 480;
            if beat % 2 == 1 {
                // pulso de tresillos
                let items: Vec<(u32, u32, Vec<Pitch>, bool)> = if rnd(3) == 0 {
                    vec![(0, 2, vec![p(step(&mut idx, &mut rnd))], false), (2, 1, vec![p(step(&mut idx, &mut rnd))], false)]
                } else {
                    (0..3).map(|k| (k, 1, vec![p(step(&mut idx, &mut rnd))], false)).collect()
                };
                s.put_triplet_beat(0, 0, at, &items).unwrap();
            } else {
                s.put_abs(0, at, 480, &[p(step(&mut idx, &mut rnd))]).unwrap();
            }
        }
    }
    s
}

/// Pieza sintética con grupos de `slots` notas por pulso (3, 5 o 6) en los pulsos 2 y 4 de cada
/// compás, sobre un bajo en negras; en algunos pulsos la primera nota es larga.
fn synthetic_tuplets(bars: u32, slots: u32) -> nova_core::Score {
    use nova_core::{Clef, Pitch, Score, TimeSig};
    let mut s = Score::new("grupos", TimeSig { num: 4, den: 4 }, 0, 1);
    let lh = s.add_part("Bajo", Clef::Bass, 0, false);
    let p = |m: i32| Pitch::from_midi(m, true);
    let scale = [60, 62, 64, 67, 69, 72, 74, 76];
    let mut seed = 4242u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 16) % 3
    };
    let mut idx = 3i32;
    for bar in 0..bars {
        let base = bar * 1920;
        let root = [48, 43, 45, 41][(bar % 4) as usize];
        for q in 0..4 {
            s.put_abs(lh, base + q * 480, 480, &[p(root + [0, 7, 12, 7][q as usize])]).unwrap();
        }
        for beat in 0..4u32 {
            let at = base + beat * 480;
            let mut next = |idx: &mut i32| {
                *idx = (*idx + rnd() as i32 - 1).clamp(0, 7);
                p(scale[*idx as usize])
            };
            if beat % 2 == 1 {
                let items: Vec<(u32, u32, Vec<Pitch>, bool)> = if bar % 3 == 2 {
                    vec![(0, 2, vec![next(&mut idx)], false), (2, slots - 2, vec![next(&mut idx)], false)]
                        .into_iter()
                        .filter(|i| i.1 > 0)
                        .collect()
                } else {
                    (0..slots).map(|k| (k, 1, vec![next(&mut idx)], false)).collect()
                };
                s.put_tuplet_beat(0, 0, at, slots, &items).unwrap();
            } else {
                s.put_abs(0, at, 480, &[next(&mut idx)]).unwrap();
            }
        }
    }
    s
}

fn main() {
    let rubato = env_f("NOVA_RUBATO", 0.0);
    let period_beats = env_f("NOVA_PERIOD", 16.0);
    let min_amp = env_f("NOVA_MINAMP", 0.5) as f32;
    let tight = env_f("NOVA_TIGHT", 30.0);
    let (mut sum_c, mut sum_f, mut cnt) = (0.0, 0.0, 0);
    for path in std::env::args().skip(1) {
        let src = if path.starts_with("@quint") {
            synthetic_tuplets(path.trim_start_matches("@quint").parse().unwrap_or(32), 5)
        } else if path.starts_with("@sext") {
            synthetic_tuplets(path.trim_start_matches("@sext").parse().unwrap_or(32), 6)
        } else if path.starts_with("@triplets") {
            synthetic_triplets(path.trim_start_matches("@triplets").parse().unwrap_or(32))
        } else if path.starts_with("@synthetic") {
            synthetic(path.trim_start_matches("@synthetic").parse().unwrap_or(48))
        } else {
            import_musicxml(&std::fs::read_to_string(&path).unwrap()).unwrap().score
        };
        let tempo = env_f("NOVA_BPM", src.tempo as f64);
        let snd: Vec<_> = sounds(&src).into_iter().filter(|s| !src.parts[s.part].percussion).collect();
        if snd.len() < 8 {
            continue;
        }
        // Tiempo (s) en función de la posición en pulsos, con tempo variable:
        // bpm(b) = tempo · (1 + rubato · sin(2π b / periodo))
        let total_beats = snd.iter().map(|s| (s.start + s.ticks) as f64 / 480.0).fold(0.0, f64::max) + 1.0;
        let step = 1.0 / 96.0;
        let mut cum = vec![0.0f64];
        let mut b = 0.0;
        while b < total_beats + 2.0 {
            let bpm = tempo * (1.0 + rubato * (std::f64::consts::TAU * (b + step / 2.0) / period_beats).sin());
            cum.push(cum.last().unwrap() + step * 60.0 / bpm);
            b += step;
        }
        let sec = |ticks: f64| -> f64 {
            let pos = ticks / 480.0 / step;
            let i = (pos.floor() as usize).min(cum.len() - 2);
            cum[i] + (pos - i as f64) * (cum[i + 1] - cum[i])
        };
        let notes: Vec<(f64, f64, i32)> =
            snd.iter().map(|s| (sec(s.start as f64), sec((s.start + s.ticks) as f64) - sec(s.start as f64), s.midi)).collect();
        let total = notes.iter().map(|n| n.0 + n.1).fold(0.0, f64::max);
        let audio = synth(&notes, total);
        let found = nova_audio::transcribe_samples(&audio, &nova_audio::Params::default()).unwrap();
        let raw: Vec<RawNote> =
            found.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset }).collect();

        if std::env::var("NOVA_DEBUG").is_ok() {
            let on: Vec<(f64, f32)> = raw.iter().filter(|n| n.amplitude >= min_amp).map(|n| (n.start, n.amplitude)).collect();
            let est = nova_core::beats::estimate_period(&on);
            let true_beats = total_beats - 1.0;
            let truth_period = (sec(true_beats * 480.0) - sec(0.0)) / true_beats;
            println!("  verdad: {:.0} pulsos, periodo medio {:.3}s | ACF: {:?}", true_beats, truth_period, est);
            if let Some((per, _)) = est {
                let b = nova_core::beats::track_beats_refined(&on, per, tight);
                let d: Vec<f64> = b.windows(2).map(|w| w[1] - w[0]).collect();
                println!("  seguidos: {} pulsos, intervalo min {:.3} max {:.3} s", b.len(), d.iter().cloned().fold(f64::MAX, f64::min), d.iter().cloned().fold(0.0, f64::max));
            }
        }
        let t0 = snd.iter().map(|s| s.start).min().unwrap() as i64;
        let reference: BTreeSet<(i32, i64)> = snd.iter().map(|s| (s.midi, s.start as i64 - t0)).collect();

        let mut res = Vec::new();
        for follow in [false, true] {
            let qo = QuantizeOptions { min_amplitude: min_amp, min_relative: if std::env::var("NOVA_MINAMP").is_ok() { None } else { Some(env_f("NOVA_REL", 0.65) as f32) }, follow_tempo: follow, tightness: tight, clean: env_f("NOVA_CLEAN", 1.0) > 0.5, triplets: env_f("NOVA_TRIPLETS", 1.0) > 0.5, ..Default::default() };
            let r = match notes_to_score(&raw, &qo) {
                Ok(r) => r,
                Err(e) => {
                    println!("{path}: ERROR {e}");
                    continue;
                }
            };
            if follow && std::env::var("NOVA_WARN").is_ok() {
                for w in r.warnings.iter().filter(|w| w.contains("irregulares") || w.contains("rejilla")) {
                    println!("  aviso: {w}");
                }
            }
            let out = sounds(&r.score);
            let o0 = out.iter().map(|s| s.start).min().unwrap() as i64;
            // El pulso hallado puede ser la mitad o el doble del original: se prueba cada escala.
            let mut best = (0.0, 0.0, 0.0, 1.0);
            for scale in [0.5, 1.0, 2.0] {
                let got: BTreeSet<(i32, i64)> =
                    out.iter().map(|s| (s.midi, ((s.start as i64 - o0) as f64 * scale).round() as i64)).collect();
                let (p, rc, f) = f1(&reference, &got, env_f("NOVA_TOL", 60.0) as i64);
                if f > best.2 {
                    best = (p, rc, f, scale);
                }
            }
            res.push((best, r.score.tempo));
        }
        if res.len() == 2 {
            let ((pc, rc, fc, sc), tc) = res[0];
            let ((pf, rf, ff, sf), tf) = res[1];
            println!(
                "{:<20} ref {:>3} | constante: tempo {:>3} esc {:.1} P {:.2} R {:.2} F1 {:.2} | siguiendo: tempo {:>3} esc {:.1} P {:.2} R {:.2} F1 {:.2}",
                std::path::Path::new(&path).file_name().unwrap().to_string_lossy(),
                reference.len(), tc, sc, pc, rc, fc, tf, sf, pf, rf, ff
            );
            sum_c += fc;
            sum_f += ff;
            cnt += 1;
        }
    }
    if cnt > 0 {
        println!(
            "MEDIA F1 posición exacta: constante {:.3} | siguiendo {:.3}  (rubato {rubato}, {cnt} muestras)",
            sum_c / cnt as f64,
            sum_f / cnt as f64
        );
    }
}
