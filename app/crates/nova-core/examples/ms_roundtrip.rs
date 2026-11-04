//! Developer tool: write a score as `.mscz`, let MuseScore itself convert it to MusicXML and
//! compare the notes MuseScore sees with the ones we wrote.
//!
//! `cargo run --release --example ms_roundtrip -- input.(mscz|mscx|musicxml) out.mscz`
use nova_core::io::{import_musicxml, sounds, Sound};
use nova_core::mscx::import_mscz;
use nova_core::mscx_write::export_mscz;

fn load(path: &str) -> nova_core::Score {
    let bytes = std::fs::read(path).expect("cannot read input");
    let lower = path.to_lowercase();
    if lower.ends_with(".musicxml") || lower.ends_with(".xml") {
        import_musicxml(&String::from_utf8_lossy(&bytes)).expect("musicxml import").score
    } else if lower.ends_with(".nova") {
        serde_json::from_slice(&bytes).expect("nova json")
    } else {
        import_mscz(&bytes).expect("mscz import").score
    }
}

fn key(s: &Sound) -> (usize, u32, u32, i32) {
    (s.part, s.start, s.ticks, s.midi)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, out) = (&args[1], &args[2]);
    let a = load(input);
    let stem = std::path::Path::new(out).file_stem().unwrap().to_string_lossy().to_string();
    std::fs::write(out, export_mscz(&a, &stem).expect("export")).expect("write mscz");
    let xml_path = format!("{out}.musicxml");
    let ms = std::env::var("MUSESCORE").unwrap_or_else(|_| r"C:\Program Files\MuseScore 4\bin\MuseScore4.exe".into());
    let t = std::time::Instant::now();
    let status = std::process::Command::new(&ms).args(["-o", &xml_path, out]).status().expect("cannot run MuseScore");
    println!("MuseScore exit: {status} in {:?}", t.elapsed());
    let b = import_musicxml(&std::fs::read_to_string(&xml_path).expect("MuseScore produced no MusicXML")).expect("MuseScore's MusicXML").score;
    println!("ours:      {} parts, {} measures, time {}/{} changes {}", a.parts.len(), a.parts[0].measures.len(), a.time.num, a.time.den, a.time_changes.len());
    println!("MuseScore: {} parts, {} measures, time {}/{} changes {}", b.parts.len(), b.parts[0].measures.len(), b.time.num, b.time.den, b.time_changes.len());
    let (mut sa, mut sb): (Vec<_>, Vec<_>) = (sounds(&a).iter().map(key).collect(), sounds(&b).iter().map(key).collect());
    sa.sort();
    sb.sort();
    println!("sounds: ours {} / MuseScore {}", sa.len(), sb.len());
    let only_a: Vec<_> = sa.iter().filter(|x| !sb.contains(x)).take(8).collect();
    let only_b: Vec<_> = sb.iter().filter(|x| !sa.contains(x)).take(8).collect();
    println!("only ours (first 8): {only_a:?}\nonly MuseScore (first 8): {only_b:?}");
    let spans = |s: &nova_core::Score| {
        let mut v: Vec<String> = s.parts.iter().enumerate().flat_map(|(i, p)| p.spanners.iter().map(move |x| format!("{i}:{:?}:v{}:{}.{}-{}.{}", x.kind, x.voice, x.start.measure, x.start.tick, x.end.measure, x.end.tick))).collect();
        v.sort();
        v
    };
    let (pa, pb) = (spans(&a), spans(&b));
    println!("spanners: ours {} / MuseScore {}", pa.len(), pb.len());
    if pa != pb {
        println!("only ours: {:?}
only MuseScore: {:?}", pa.iter().filter(|x| !pb.contains(x)).take(6).collect::<Vec<_>>(), pb.iter().filter(|x| !pa.contains(x)).take(6).collect::<Vec<_>>());
    }
    let marks = |s: &nova_core::Score| {
        let mut v: Vec<String> = Vec::new();
        for (pi, p) in s.parts.iter().enumerate() {
            for (mi, m) in p.measures.iter().enumerate() {
                for vi in 0..4 {
                    for e in m.voice(vi).into_iter().flatten() {
                        if e.dynamic.is_some() || !e.articulations.is_empty() {
                            let mut ar: Vec<_> = e.articulations.iter().map(|a| a.name()).collect();
                            ar.sort();
                            v.push(format!("{pi}:{mi}:{vi}:{}:{:?}:{ar:?}", e.start, e.dynamic.map(|d| d.name())));
                        }
                    }
                }
            }
        }
        v.sort();
        v
    };
    let reps = |s: &nova_core::Score| -> Vec<String> {
        s.parts.iter().enumerate().flat_map(|(pi, p)| p.measures.iter().enumerate().filter(|(_, m)| m.start_repeat || m.end_repeat > 0).map(move |(mi, m)| format!("{pi}:{mi}:{}:{}", m.start_repeat, m.end_repeat))).collect()
    };
    let (ra, rb) = (reps(&a), reps(&b));
    println!("repeat signs: ours {} / MuseScore {} {}", ra.len(), rb.len(), if ra == rb { "(identical)" } else { "(DIFFERENT)" });
    if ra != rb { println!("ours {ra:?}
MuseScore {rb:?}"); }
    let (ma, mb) = (marks(&a), marks(&b));
    println!("dynamics/articulations: ours {} / MuseScore {}", ma.len(), mb.len());
    if ma != mb {
        println!("only ours: {:?}
only MuseScore: {:?}", ma.iter().filter(|x| !mb.contains(x)).take(6).collect::<Vec<_>>(), mb.iter().filter(|x| !ma.contains(x)).take(6).collect::<Vec<_>>());
    }
    println!("{}{}{}", if sa == sb { "IDENTICAL NOTES" } else { "DIFFERENT" }, if pa == pb { " + IDENTICAL SPANNERS" } else { " / DIFFERENT SPANNERS" }, if ma == mb { " + IDENTICAL MARKS" } else { " / DIFFERENT MARKS" });
}
