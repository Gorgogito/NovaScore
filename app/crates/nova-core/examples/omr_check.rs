//! Developer tool: how faithful is the OMR result? Compares the sounding notes of the original MusicXML
//! with those of the file Audiveris produced from a printed copy of it (`.mxl` or `.xml`).
//!
//! `cargo run --release --example omr_check -- samples/demo.musicxml resultado.mxl`
use nova_core::io::{import_musicxml, import_musicxml_bytes, sounds};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let a = import_musicxml(&std::fs::read_to_string(&args[1]).expect("original")).expect("original musicxml").score;
    let r = import_musicxml_bytes(&std::fs::read(&args[2]).expect("resultado")).expect("resultado musicxml");
    let b = r.score;
    let flat = |s: &nova_core::Score| {
        let mut v: Vec<(u32, u32, i32)> = sounds(s).iter().map(|x| (x.start, x.ticks, x.midi)).collect();
        v.sort();
        v
    };
    let (fa, fb) = (flat(&a), flat(&b));
    let onset = |v: &[(u32, u32, i32)]| v.iter().map(|x| (x.0, x.2)).collect::<Vec<_>>();
    let (oa, ob) = (onset(&fa), onset(&fb));
    let pitch_hit = oa.iter().filter(|x| ob.contains(x)).count();
    let exact = fa.iter().filter(|x| fb.contains(x)).count();
    let measures = |s: &nova_core::Score| s.parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    println!(
        "{}: original {} notas / OMR {} notas | partes {}→{} | compases {}→{} | tonalidad {}→{} | compás {}/{}→{}/{}",
        args[1].rsplit(['/', '\\']).next().unwrap_or(""),
        fa.len(),
        fb.len(),
        a.parts.len(),
        b.parts.len(),
        measures(&a),
        measures(&b),
        a.fifths,
        b.fifths,
        a.time.num,
        a.time.den,
        b.time.num,
        b.time.den
    );
    println!(
        "  inicio+altura: {pitch_hit}/{} ({:.0} %) · inicio+duración+altura: {exact}/{} ({:.0} %) · sobran {}",
        fa.len(),
        100.0 * pitch_hit as f64 / fa.len().max(1) as f64,
        fa.len(),
        100.0 * exact as f64 / fa.len().max(1) as f64,
        ob.iter().filter(|x| !oa.contains(x)).count()
    );
    for w in r.warnings.iter().take(3) {
        println!("  ! {w}");
    }
}
