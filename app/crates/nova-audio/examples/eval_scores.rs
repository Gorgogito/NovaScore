//! Evaluación de extremo a extremo con verdad conocida: partitura (.mscz) → audio con el SoundFont
//! de MuseScore (oxisynth) → transcripción → partitura → comparación con la original.
//! Mide tempo (módulo octavas), compás, tonalidad y F1 de notas (altura + tick, relativo a la
//! primera nota). El audio no tiene rubato (el tempo de la partitura es constante).
//!
//! Uso: cargo run --release --example eval_scores -- a.mscz b.mscz …
//! Variables: NOVA_PIANO=modelo.onnx (motor de piano; si no, Basic Pitch), NOVA_BARS=32 (compases),
//!   NOVA_SF=ruta.sf3, NOVA_TOL=60, NOVA_REL=0.5 (umbral relativo, solo Basic Pitch), NOVA_NOAUTO=1
//!   (compás 4/4 fijo sin autodetección), NOVA_PROGRAM=0 (fuerza un instrumento GM), NOVA_VERBOSE=1,
//!   NOVA_RUBATO=0.15 (amplitud del rubato sinusoidal; 0 = sin rubato) y NOVA_PERIOD=16 (pulsos por ciclo).
use nova_audio::piano::{self, Device};
use nova_core::io::sounds;
use nova_core::mscx::import_mscz;
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};
use oxisynth::{MidiEvent, SoundFont, Synth, SynthDescriptor};
use std::collections::BTreeSet;
use std::path::Path;

const SR: usize = 22050;

fn env_f(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn render(notes: &[(f64, f64, u8, u8, u8)], programs: &[(u8, u8)], sf: &Path) -> Vec<f32> {
    let desc = SynthDescriptor { sample_rate: SR as f32, reverb_active: false, chorus_active: false, ..Default::default() };
    let mut synth = Synth::new(desc).unwrap();
    let mut file = std::fs::File::open(sf).expect("SoundFont");
    synth.add_font(SoundFont::load(&mut file).expect("cargar SoundFont"), true);
    for &(ch, prog) in programs {
        synth.send_event(MidiEvent::ProgramChange { channel: ch, program_id: prog }).unwrap();
    }
    // (muestra, inicio?, canal, tecla, velocidad)
    let mut ev: Vec<(usize, bool, u8, u8, u8)> = Vec::new();
    for &(s, d, ch, key, vel) in notes {
        ev.push(((s * SR as f64) as usize, true, ch, key, vel));
        ev.push((((s + d) * SR as f64) as usize, false, ch, key, 0));
    }
    ev.sort_by_key(|e| (e.0, e.1));
    let total = ev.last().map_or(0, |e| e.0) + 2 * SR;
    let mut out = Vec::with_capacity(total);
    let mut k = 0;
    for i in 0..total {
        while k < ev.len() && ev[k].0 <= i {
            let e = ev[k];
            let m = if e.1 { MidiEvent::NoteOn { channel: e.2, key: e.3, vel: e.4 } } else { MidiEvent::NoteOff { channel: e.2, key: e.3 } };
            let _ = synth.send_event(m);
            k += 1;
        }
        let (l, r) = synth.read_next();
        out.push(0.5 * (l + r));
    }
    let peak = out.iter().fold(0.0f32, |a, &b| a.max(b.abs())).max(1e-6);
    out.iter_mut().for_each(|v| *v *= 0.8 / peak);
    out
}

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

fn main() {
    let sf = std::env::var("NOVA_SF").unwrap_or_else(|_| "../../public/soundfonts/MuseScore_General.sf3".into());
    let piano_model = std::env::var("NOVA_PIANO").ok();
    let bars = env_f("NOVA_BARS", 32.0) as u32;
    let tol = env_f("NOVA_TOL", 60.0) as i64;
    let force_program = std::env::var("NOVA_PROGRAM").ok().and_then(|s| s.parse::<u8>().ok());
    let (mut n, mut sf1, mut tempo_ok, mut meter_ok, mut key_ok) = (0, 0.0, 0, 0, 0);
    println!("{:<34} {:>22} {:>13} {:>9} {:>20}", "partitura", "tempo (ref → est, módulo 2^k)", "compás", "tonalidad", "notas P / R / F1");
    for path in std::env::args().skip(1) {
        let imported = if path.ends_with(".musicxml") || path.ends_with(".xml") {
            nova_core::io::import_musicxml(&std::fs::read_to_string(&path).unwrap())
        } else {
            import_mscz(&std::fs::read(&path).unwrap())
        };
        let sc = match imported {
            Ok(r) => r.score,
            Err(e) => {
                println!("{path}: {e}");
                continue;
            }
        };
        let spt = 60.0 / sc.tempo as f64 / 480.0;
        let limit = bars * sc.time.measure_ticks();
        let snd: Vec<_> = sounds(&sc).into_iter().filter(|s| !sc.parts[s.part].percussion && s.start < limit).collect();
        if snd.len() < 16 {
            continue;
        }
        // Rubato: bpm(b) = tempo · (1 + a · sin(2π b / periodo)); sec(ticks) = tiempo acumulado.
        let rubato = env_f("NOVA_RUBATO", 0.0);
        let period_beats = env_f("NOVA_PERIOD", 16.0);
        let step = 1.0 / 96.0;
        let max_beats = snd.iter().map(|s| (s.start + s.ticks) as f64 / 480.0).fold(0.0, f64::max) + 2.0;
        let mut cum = vec![0.0f64];
        let mut b = 0.0;
        while b < max_beats + 1.0 {
            let bpm = sc.tempo as f64 * (1.0 + rubato * (std::f64::consts::TAU * (b + step / 2.0) / period_beats).sin());
            cum.push(cum.last().unwrap() + step * 60.0 / bpm);
            b += step;
        }
        let sec = |ticks: f64| -> f64 {
            if rubato == 0.0 {
                return ticks * spt;
            }
            let x = (ticks / 480.0 / step).max(0.0);
            let i = (x.floor() as usize).min(cum.len() - 2);
            cum[i] + (x - i as f64) * (cum[i + 1] - cum[i])
        };
        let notes: Vec<(f64, f64, u8, u8, u8)> = snd
            .iter()
            .map(|s| (1.0 + sec(s.start as f64), sec((s.start + s.ticks) as f64) - sec(s.start as f64), s.channel, s.midi.clamp(0, 127) as u8, s.velocity.max(40)))
            .collect();
        let stem = Path::new(&path).file_stem().unwrap().to_string_lossy().into_owned();
        let dir = format!("target/evalset/{}", if piano_model.is_some() { "piano" } else { "bp" });
        let cpath = format!("{dir}/{stem}-r{rubato}.tsv");
        let cached: Option<Vec<RawNote>> = if std::env::var("NOVA_CACHED").is_ok() {
            std::fs::read_to_string(&cpath).ok().map(|t| {
                t.lines()
                    .filter_map(|l| {
                        let v: Vec<&str> = l.split('\t').collect();
                        Some(RawNote { start: v.first()?.parse().ok()?, end: v.get(1)?.parse().ok()?, pitch: v.get(2)?.parse().ok()?, amplitude: v.get(3)?.parse().ok()?, onset: *v.get(4)? == "1" })
                    })
                    .collect()
            })
        } else {
            None
        };
        let raw: Vec<RawNote> = match cached {
            Some(r) => r,
            None => {
            let mut programs: Vec<(u8, u8)> = snd.iter().map(|s| (s.channel, force_program.unwrap_or(sc.parts[s.part].program))).collect();
            programs.sort();
            programs.dedup();
            let audio = render(&notes, &programs, Path::new(&sf));
            if let Ok(wd) = std::env::var("NOVA_WAV") {
                // guarda el audio sintético (PCM 16 bits mono) para probarlo a mano en la app
                let mut b: Vec<u8> = Vec::with_capacity(44 + audio.len() * 2);
                let data = (audio.len() * 2) as u32;
                b.extend_from_slice(b"RIFF");
                b.extend_from_slice(&(36 + data).to_le_bytes());
                b.extend_from_slice(b"WAVEfmt ");
                b.extend_from_slice(&16u32.to_le_bytes());
                b.extend_from_slice(&1u16.to_le_bytes());
                b.extend_from_slice(&1u16.to_le_bytes());
                b.extend_from_slice(&(SR as u32).to_le_bytes());
                b.extend_from_slice(&(SR as u32 * 2).to_le_bytes());
                b.extend_from_slice(&2u16.to_le_bytes());
                b.extend_from_slice(&16u16.to_le_bytes());
                b.extend_from_slice(b"data");
                b.extend_from_slice(&data.to_le_bytes());
                for v in &audio {
                    b.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                }
                let _ = std::fs::create_dir_all(&wd);
                let _ = std::fs::write(format!("{wd}/{stem}-r{rubato}.wav"), b);
            }

            let found = match &piano_model {
                Some(m) => {
                    let x = {
                        // 22,05 kHz → 16 kHz por interpolación lineal (basta para esta prueba)
                        let ratio = 16000.0 / SR as f64;
                        (0..(audio.len() as f64 * ratio) as usize)
                            .map(|i| {
                                let p = i as f64 / ratio;
                                let a = p.floor() as usize;
                                let f = (p - a as f64) as f32;
                                audio[a] * (1.0 - f) + audio[(a + 1).min(audio.len() - 1)] * f
                            })
                            .collect::<Vec<f32>>()
                    };
                    piano::transcribe_samples(Path::new(m), Device::Auto, &x, &Default::default()).unwrap()
                }
                None => nova_audio::transcribe_samples(&audio, &nova_audio::Params::default()).unwrap(),
            };
            let raw: Vec<RawNote> = found.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset }).collect();
                // caché de las notas transcritas (por rubato), para experimentos sin repetir el audio (NOVA_CACHED=1)
                std::fs::create_dir_all(&dir).unwrap();
                let mut t = format!("# fifths {}
# bars {}
", sc.fifths, bars);
                for n in &raw {
                    t += &format!("{}	{}	{}	{}	{}
", n.start, n.end, n.pitch, n.amplitude, n.onset as u8);
                }
                std::fs::write(&cpath, t).unwrap();
                raw
            }
        };
        let mut qo = QuantizeOptions { auto_meter: true, auto_pickup: true, compound: true, robust_tempo: true, ..Default::default() };
        if std::env::var("NOVA_NOAUTO").is_ok() {
            qo.auto_meter = false;
            qo.auto_pickup = false;
        }
        // pistas del usuario: NOVA_HINT_TEMPO=183 y NOVA_HINT_METER=3/4 se aplican a todas las piezas
        if let Some(v) = std::env::var("NOVA_HINT_TEMPO").ok().and_then(|s| s.parse::<f64>().ok()) {
            qo.tempo = Some(v);
        }
        if let Some((n, d)) = std::env::var("NOVA_HINT_METER").ok().and_then(|s| s.split_once('/').map(|(a, b)| (a.parse::<u32>().ok(), b.parse::<u32>().ok()))).and_then(|(a, b)| Some((a?, b?))) {
            qo.time = nova_core::TimeSig { num: n, den: d };
            qo.auto_meter = false;
        }
        if let Some(v) = std::env::var("NOVA_HARM").ok().and_then(|s| s.parse().ok()) {
            qo.clean_params.harmonic_ratio = v;
        }
        if let Some(v) = std::env::var("NOVA_CHROMA").ok().and_then(|s| s.parse().ok()) {
            qo.clean_params.chromatic_pct = v;
        }
        if let Some(v) = std::env::var("NOVA_MARGIN").ok().and_then(|s| s.parse().ok()) {
            qo.clean_params.key_min_margin = v;
        }
        qo.lax_tightness = std::env::var("NOVA_LAX").ok().and_then(|s| s.parse().ok()).or(Some(4.0)).filter(|&v| v > 0.0);
        if let Some(v) = std::env::var("NOVA_TIGHT").ok().and_then(|s| s.parse().ok()) {
            qo.tightness = v;
        }
        if std::env::var("NOVA_NOTUPLETS").is_ok() {
            qo.triplets = false;
        }
        if std::env::var("NOVA_NOCLEAN").is_ok() {
            qo.clean = false;
        }
        if piano_model.is_some() {
            qo.min_relative = None;
            qo.min_amplitude = 0.0;
        } else {
            qo.min_relative = Some(env_f("NOVA_REL", 0.5) as f32);
        }
        let r = match notes_to_score(&raw, &qo) {
            Ok(r) => r,
            Err(e) => {
                println!("{path}: ERROR {e}");
                continue;
            }
        };
        if std::env::var("NOVA_VERBOSE").is_ok() {
            for w in &r.warnings {
                println!("    · {w}");
            }
        }
        // tempo módulo octavas
        // con rubato sinusoidal el tempo medio (en pulsos) es tempo·√(1−a²)
        let mean_tempo = sc.tempo as f64 * (1.0 - rubato * rubato).sqrt();
        let ratio = r.score.tempo as f64 / mean_tempo;
        let k = ratio.log2().round();
        let err = ratio / 2f64.powf(k) - 1.0;
        let tempo_good = err.abs() < 0.04;
        // compás
        let truth_t = format!("{}/{}", sc.time.num, sc.time.den);
        let est_t = format!("{}/{}", r.score.time.num, r.score.time.den);
        // notas
        let t0 = snd.iter().map(|s| s.start).min().unwrap() as i64;
        let reference: BTreeSet<(i32, i64)> = snd.iter().map(|s| (s.midi, s.start as i64 - t0)).collect();
        let out = sounds(&r.score);
        let o0 = out.iter().map(|s| s.start).min().unwrap_or(0) as i64;
        if std::env::var("NOVA_VERBOSE").is_ok() {
            let g: Vec<_> = out.iter().map(|s| (s.midi, s.start as i64 - o0)).collect::<BTreeSet<_>>().into_iter().take(10).collect();
            println!("    ref {} notas {:?}
    got {} notas {:?} (detectadas {})", reference.len(), reference.iter().take(10).collect::<Vec<_>>(), out.len(), g, raw.len());
        }
        let mut best = (0.0, 0.0, 0.0);
        let mut best_shift = 0i64;
        let ref_v: Vec<(i32, i64)> = reference.iter().cloned().collect();
        for scale in [1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0, 1.5, 2.0, 3.0] {
            let got_v: Vec<(i32, i64)> = out.iter().map(|s| (s.midi, ((s.start as i64 - o0) as f64 * scale).round() as i64)).collect();
            // la primera nota puede ser espuria o faltar: se prueba también el desplazamiento global más votado
            // por las diferencias de tiempo entre notas de igual altura
            let mut votes: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
            for &(m, t) in ref_v.iter().filter(|x| x.1 < 9600).take(200) {
                for &(gm, gt) in got_v.iter().filter(|x| x.1 < 14400) {
                    if gm == m && (gt - t).abs() <= 4800 {
                        *votes.entry((gt - t) / 20 * 20).or_default() += 1;
                    }
                }
            }
            let mut cands: Vec<(i64, usize)> = votes.into_iter().collect();
            cands.sort_by_key(|c| std::cmp::Reverse(c.1));
            let mut shifts: Vec<i64> = vec![0];
            shifts.extend(cands.iter().take(5).map(|c| c.0));
            for sh in shifts {
                let got: BTreeSet<(i32, i64)> = got_v.iter().map(|&(m, t)| (m, t - sh)).collect();
                let x = f1(&reference, &got, tol);
                if x.2 > best.2 {
                    best = x;
                    best_shift = sh;
                }
            }
        }
        if std::env::var("NOVA_VERBOSE").is_ok() && best_shift != 0 {
            println!("    desplazamiento global de las notas: {best_shift} ticks (la primera nota no coincide)");
        }
        println!(
            "{:<34} {:>3} → {:>3} (×{}, {:+.1} %) {:>3} {:>6}→{:<6} {:>4}→{:<4} {:.2} / {:.2} / {:.2}",
            Path::new(&path).file_stem().unwrap().to_string_lossy().chars().take(33).collect::<String>(),
            sc.tempo,
            r.score.tempo,
            2f64.powf(k),
            err * 100.0,
            if tempo_good { "✓" } else { "✗" },
            truth_t,
            est_t,
            sc.fifths,
            r.score.fifths,
            best.0,
            best.1,
            best.2
        );
        // acordes: verdad de `artefactos/acordes/<stem>.json` (si existe) frente a los detectados en la partitura transcrita
        let truth_path = format!("../../../artefactos/acordes/{stem}.json");
        if let Ok(txt) = std::fs::read_to_string(&truth_path) {
            let truth: Vec<serde_json::Value> = serde_json::from_str(&txt).unwrap_or_default();
            let est_marks = nova_core::chords::detect_chords(&r.score);
            let n_o = sc.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
            let _ = n_o;
            let first_repeat_end = sc.parts[0].measures.iter().position(|m| m.end_repeat > 0).unwrap_or(usize::MAX);
            // tiempo de reproducción (con repeticiones desplegadas) de la primera vez que suena cada compás
            let order = sc.playback_order();
            let play_start = |m: usize| -> Option<u32> {
                let mut pos = 0;
                for &x in &order {
                    if x == m {
                        return Some(pos);
                    }
                    pos += sc.measure_len(x);
                }
                None
            };
            let n_e = r.score.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
            let estarts = r.score.measure_starts(n_e);
            let est_spt = 60.0 / r.score.tempo as f64 / 480.0;
            // acorde detectado vigente en el instante t (s desde la primera nota)
            let cshift = env_f("NOVA_CSHIFT", 0.0);
            let est_at = |t: f64| est_marks.iter().filter(|c| (estarts[c.measure] + c.tick).saturating_sub(o0 as u32) as f64 * est_spt <= t + 0.05 + cshift).last();
            let (mut cn, mut cr, mut ct) = (0, 0, 0);
            for t in &truth {
                let kind = t["kind"].as_str().unwrap_or("");
                let tri = match kind { "major" | "dominant" | "major-seventh" => 0, "minor" | "minor-seventh" => 1, "diminished" | "diminished-seventh" => 2, _ => continue };
                let (m, tick, root) = (t["measure"].as_u64().unwrap() as usize, t["tick"].as_u64().unwrap() as u32, t["root"].as_u64().unwrap() as u8);
                if m > first_repeat_end {
                    continue; // tras el primer final de repetición el tiempo de reproducción ya no coincide con el escrito
                }
                let Some(ps) = play_start(m) else { continue };
                let abs = ps + tick;
                if abs >= limit || (abs as i64) < t0 {
                    continue;
                }
                cn += 1;
                if std::env::var("NOVA_VERBOSE").is_ok() {
                    let g = est_at((abs as i64 - t0) as f64 * spt);
                    println!("      comp {} +{} t={:.1}s verdad {}{} → {}", m + 1, tick, (abs as i64 - t0) as f64 * spt, root, kind, g.map_or("-".into(), |c| c.name(0)));
                }
                if let Some(c) = est_at((abs as i64 - t0) as f64 * spt) {
                    cr += (c.root == root) as i32;
                    ct += (c.root == root && c.quality.triad() == tri) as i32;
                }
            }
            if cn > 0 {
                println!("    acordes: {cn} de la verdad, fundamental {:.0} %, tríada {:.0} %", 100.0 * cr as f64 / cn as f64, 100.0 * ct as f64 / cn as f64);
            }
        }
        n += 1;
        sf1 += best.2;
        tempo_ok += tempo_good as i32;
        meter_ok += (truth_t == est_t) as i32;
        key_ok += (sc.fifths == r.score.fifths) as i32;
    }
    if n > 0 {
        println!("\n{n} piezas: tempo correcto {tempo_ok}/{n}, compás {meter_ok}/{n}, tonalidad {key_ok}/{n}, F1 medio {:.3}", sf1 / n as f64);
    }
}
