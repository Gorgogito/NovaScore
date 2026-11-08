//! Diagnóstico de una grabación: distribución de amplitudes y puntuación de tempos.
//! Uso: cargo run --release --example analyze -- audio.mp3
use nova_core::quantize::{estimate_beat, estimate_key, RawNote};

fn main() {
    let path = std::env::args().nth(1).expect("falta el audio");
    let notes = nova_audio::transcribe_file(std::path::Path::new(&path), &nova_audio::Params::default()).unwrap();
    let dur = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    println!("{} notas, {:.1} s", notes.len(), dur);
    let mut h = [0usize; 10];
    for n in &notes {
        h[((n.amplitude * 10.0) as usize).min(9)] += 1;
    }
    println!("amplitud (0.0-0.1 … 0.9-1.0): {h:?}");
    let mut pitches = [0usize; 12];
    for n in &notes {
        pitches[(n.pitch % 12) as usize] += 1;
    }
    println!("clases de altura C..B: {pitches:?}");
    let lens: Vec<f64> = notes.iter().map(|n| n.end - n.start).collect();
    println!("duración media {:.2} s, mediana {:.2} s", lens.iter().sum::<f64>() / lens.len() as f64, {
        let mut l = lens.clone();
        l.sort_by(|a, b| a.partial_cmp(b).unwrap());
        l[l.len() / 2]
    });
    for min in [0.0f32, 0.3, 0.4, 0.5] {
        let on: Vec<(f64, f32)> = notes.iter().filter(|n| n.amplitude >= min).map(|n| (n.start, n.amplitude)).collect();
        let b = estimate_beat(&on);
        println!("min_amp {min:.1}: {} notas → {:.1} bpm, confianza {:.2}", on.len(), b.bpm, b.confidence);
    }
    let raw: Vec<RawNote> = notes.iter().map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset }).collect();
    let (fifths, best, margin) = estimate_key(&raw);
    println!("tonalidad: {fifths} alteraciones, correlación {best:.3}, margen {margin:.3}");
    // candidatos a armónico: nota más aguda a 12/19/24/28/31 semitonos de otra que empieza a la vez
    let mut ratios = Vec::new();
    for n in &notes {
        for l in &notes {
            if n.pitch > l.pitch && [12, 19, 24, 28, 31].contains(&(n.pitch - l.pitch)) && (n.start - l.start).abs() < 0.08 {
                ratios.push((n.amplitude / l.amplitude, n.pitch - l.pitch));
            }
        }
    }
    let mut hist = [0usize; 8];
    for (r, _) in &ratios {
        hist[((r * 5.0) as usize).min(7)] += 1;
    }
    println!("pares armónico/fundamental: {} | razón de amplitud (0-0.2,0.2-0.4,…,1.4+): {hist:?}", ratios.len());
    let mut gaps = [0usize; 8];
    let mut sorted: Vec<_> = notes.iter().collect();
    sorted.sort_by(|a, b| (a.pitch, a.start.to_bits()).cmp(&(b.pitch, b.start.to_bits())));
    for w in sorted.windows(2) {
        if w[0].pitch == w[1].pitch {
            let g = w[1].start - w[0].end;
            if (-0.2..0.4).contains(&g) {
                gaps[(((g + 0.2) / 0.075) as usize).min(7)] += 1;
            }
        }
    }
    println!("huecos entre notas iguales consecutivas (-0.2..0.4 s en pasos de 75 ms): {gaps:?}");
    // intervalos entre inicios: histograma en pasos de 50 ms
    let mut starts: Vec<f64> = notes.iter().map(|n| n.start).collect();
    starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut ioi = std::collections::BTreeMap::new();
    for w in starts.windows(2) {
        let d = w[1] - w[0];
        if d > 0.04 && d < 2.0 {
            *ioi.entry((d * 20.0).round() as i32).or_insert(0usize) += 1;
        }
    }
    let mut top: Vec<_> = ioi.into_iter().collect();
    top.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("intervalos entre inicios más comunes (s, n): {:?}", top.iter().take(8).map(|(k, v)| (*k as f64 / 20.0, *v)).collect::<Vec<_>>());
}
