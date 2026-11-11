//! Comprueba la ida y vuelta del cifrado por MuseScore: MusicXML con `<harmony>` → .mscz → lectura.
//! Uso: cargo run --release --example chords_mscz -- partitura.musicxml [salida.mscz]   (no escribe si no se da salida)
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut score = nova_core::io::import_musicxml(&std::fs::read_to_string(&a[0]).unwrap()).unwrap().score;
    if std::env::var("NOVA_DETECT").is_ok() {
        score.detect_chord_symbols();
    }
    let bytes = nova_core::mscx_write::export_mscz(&score, "prueba").unwrap();
    if let Some(out) = a.get(1) {
        assert!(!std::path::Path::new(out).exists(), "la salida ya existe");
        std::fs::write(out, &bytes).unwrap();
    }
    let back = nova_core::mscx::import_mscz(&bytes).unwrap();
    // MuseScore ancla el cifrado a las notas: lo esperado es el primer acorde de cada evento, en el inicio del evento.
    let mut expected = Vec::new();
    for (mi, m) in score.parts[0].measures.iter().enumerate() {
        for e in m.voice(0).map(|v| v.as_slice()).unwrap_or(&[]) {
            if let Some(c) = score.chords.iter().find(|c| c.measure == mi && e.start <= c.tick && c.tick < e.start + e.ticks) {
                expected.push(nova_core::chords::ChordMark { tick: e.start, ..*c });
            }
        }
    }
    println!("acordes que no empiezan en una nota: {}", score.chords.len() - score.chords.iter().filter(|c| score.parts[0].measures[c.measure].voice(0).map(|v| v.as_slice()).unwrap_or(&[]).iter().any(|e| e.start == c.tick)).count());
    let same = back.score.chords == expected;
    println!("acordes: {} → {} | idénticos: {same}", score.chords.len(), back.score.chords.len());
    if !same {
        for (x, y) in expected.iter().zip(&back.score.chords).filter(|(x, y)| x != y).take(5) {
            println!("  {x:?}  ≠  {y:?}");
        }
    }
    if let Ok(m) = std::env::var("NOVA_MEASURE") {
        let m: usize = m.parse().unwrap();
        for e in score.parts[0].measures[m].voice(0).map(|v| v.as_slice()).unwrap_or(&[]) {
            println!("  orig  start {} ticks {} tuplet {:?}", e.start, e.ticks, e.tuplet.map(|t| (t.actual, t.normal)));
        }
        for e in back.score.parts[0].measures[m].voice(0).map(|v| v.as_slice()).unwrap_or(&[]) {
            println!("  vuelta start {} ticks {} tuplet {:?}", e.start, e.ticks, e.tuplet.map(|t| (t.actual, t.normal)));
        }
    }
    for w in &back.warnings {
        println!("  aviso: {w}");
    }
}
