//! Prueba del modelo de piano: cargo run --release --example piano2notes -- modelo.onnx audio.mp3 [cpu|gpu]
use nova_audio::piano::{transcribe_file, Device, PianoParams};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dev = if a.get(3).map(|s| s == "cpu").unwrap_or(false) { Device::Cpu } else { Device::Auto };
    let t = std::time::Instant::now();
    let notes = transcribe_file(std::path::Path::new(&a[1]), dev, std::path::Path::new(&a[2]), &PianoParams::default()).unwrap();
    let dur = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    println!("{:?}: {} notas, {:.1} s de audio, {:.1} s de proceso", dev, notes.len(), dur, t.elapsed().as_secs_f64());
    for n in notes.iter().take(8) {
        println!("  {:.3}-{:.3} nota {} vel {:.2}", n.start, n.end, n.pitch, n.amplitude);
    }
}
