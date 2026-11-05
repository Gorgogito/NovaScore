//! Uso: cargo run --release --example audio2notes -- archivo.wav
fn main() {
    let path = std::env::args().nth(1).expect("falta el archivo de audio");
    let t = std::time::Instant::now();
    match nova_audio::transcribe_file(std::path::Path::new(&path), &nova_audio::Params::default()) {
        Ok(n) => {
            for e in &n {
                println!("{:7.3} {:7.3} {:3} {:.2}", e.start, e.end, e.pitch, e.amplitude);
            }
            eprintln!("{} notas en {:.2?}", n.len(), t.elapsed());
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    }
}
