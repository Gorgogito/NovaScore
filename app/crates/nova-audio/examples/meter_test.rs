//! Prueba de la detección de compás y anacrusa.
//!
//! - `meter_test ref partitura.mscz …`: usa las notas de la partitura (tiempo exacto) y simula
//!   anacrusas de 0..3 tiempos quitando los primeros tiempos; informa del compás y la anacrusa detectados.
//! - `meter_test audio grabacion.mp3 …`: usa la transcripción en caché de `target/cache/` (ver `calibrate`;
//!   con NOVA_PIANO=1 la del modelo de piano).
use nova_core::io::sounds;
use nova_core::mscx::import_mscz;
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};
use std::path::Path;

fn from_cache(path: &Path) -> Vec<RawNote> {
    let suffix = if std::env::var("NOVA_PIANO").is_ok() { "-piano" } else { "" };
    let f = format!("target/cache/{}{}.tsv", path.file_stem().unwrap().to_string_lossy(), suffix);
    std::fs::read_to_string(&f)
        .unwrap_or_else(|_| panic!("sin caché {f}: ejecuta antes `calibrate`"))
        .lines()
        .filter_map(|l| {
            let v: Vec<&str> = l.split('\t').collect();
            Some(RawNote { start: v[0].parse().ok()?, end: v[1].parse().ok()?, pitch: v[2].parse().ok()?, amplitude: v[3].parse().ok()?, onset: v[4] == "1" })
        })
        .collect()
}

fn report(label: &str, raw: &[RawNote], tempo: Option<f64>) -> (String, u32) {
    let o = QuantizeOptions { auto_meter: true, auto_pickup: true, compound: true, min_relative: None, min_amplitude: 0.0, tempo, ..Default::default() };
    match notes_to_score(raw, &o) {
        Ok(r) => {
            let w = r.warnings.iter().find(|w| w.starts_with("Compás") || w.starts_with("No se pudo detectar")).cloned().unwrap_or_default();
            println!("  {label:<26} → {}/{}  {}", r.score.time.num, r.score.time.den, w);
            if std::env::var("NOVA_ALL").is_ok() {
                for x in &r.warnings {
                    println!("      · {x}");
                }
            }
            (format!("{}/{}", r.score.time.num, r.score.time.den), 0)
        }
        Err(e) => {
            println!("  {label:<26} ERROR {e}");
            (String::new(), 0)
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mode = a.get(1).map(String::as_str).unwrap_or("");
    for f in &a[2..] {
        println!("{f}");
        if mode == "ref" {
            let sc = import_mscz(&std::fs::read(f).unwrap()).unwrap().score;
            let spt = 60.0 / sc.tempo as f64 / 480.0;
            let bar = sc.time.measure_ticks() as f64 / 480.0; // negras por compás
            println!("  referencia: {}/{} a {} bpm", sc.time.num, sc.time.den, sc.tempo);
            let notes: Vec<RawNote> = sounds(&sc)
                .into_iter()
                .filter(|s| !sc.parts[s.part].percussion)
                .map(|s| RawNote { start: s.start as f64 * spt, end: (s.start + s.ticks) as f64 * spt, pitch: s.midi.clamp(0, 127) as u8, amplitude: s.velocity as f32 / 127.0, onset: true })
                .collect();
            let beat_s = 60.0 / sc.tempo as f64;
            // solo los primeros ~24 compases (acelera y evita repeticiones largas)
            for pick in 0..4u32 {
                if pick as f64 >= bar {
                    break;
                }
                let cut = (bar.floor() - pick as f64) * beat_s - 0.01;
                let sub: Vec<RawNote> = notes.iter().filter(|n| n.start >= cut && n.start < cut + 28.0 * bar * beat_s).copied().collect();
                report(&format!("anacrusa simulada {pick}"), &sub, Some(std::env::var("NOVA_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(sc.tempo as f64)));
            }
        } else {
            report("grabación", &from_cache(Path::new(f)), None);
        }
    }
}
