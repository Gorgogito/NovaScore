//! Lee un MusicXML, detecta acordes (o conserva los que trae con `NOVA_KEEP=1`) y escribe otro MusicXML con `<harmony>`.
//! Uso: cargo run --release --example chords_xml -- entrada.musicxml salida.musicxml
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 2 || std::path::Path::new(&a[1]).exists() {
        eprintln!("Uso: chords_xml entrada.musicxml salida.musicxml (la salida no debe existir)");
        std::process::exit(2);
    }
    let mut r = nova_core::io::import_musicxml(&std::fs::read_to_string(&a[0]).unwrap()).unwrap();
    println!("acordes leídos del archivo: {}", r.score.chords.len());
    if std::env::var("NOVA_KEEP").is_err() {
        r.score.detect_chord_symbols();
        println!("acordes detectados: {}", r.score.chords.len());
    }
    std::fs::write(&a[1], nova_core::io::export_musicxml(&r.score)).unwrap();
}
