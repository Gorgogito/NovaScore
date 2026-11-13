//! Developer tool: how well does `Score::harmonize` recover a known progression?
//!
//! There is no corpus of scores with human chord symbols in the repository, so the truth is synthetic: for each
//! progression a melody is generated over it (chord tones on the strong beats, chord tones or scale neighbours on the
//! weak ones, with a fixed seed) and the harmonizer must find the chords again from the melody alone. This measures
//! whether the heuristics are coherent, **not** whether the result sounds good: a melody has many valid harmonizations.
//!
//! `cargo run --release --example eval_harmonize` — optionally followed by `.mscz` / `.musicxml` files, for which it
//! only prints the chords found and the time taken (long pieces, no truth).
use nova_core::assist::HarmonyOptions;
use nova_core::chords::Quality;
use nova_core::{Pitch, Score, TimeSig};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as u32
    }
}

const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// (offset from the tonic, quality) of the roman numerals of the progressions, in C major or A minor (tonic 9).
fn progressions() -> Vec<(&'static str, bool, Vec<(i32, Quality)>)> {
    use Quality::*;
    vec![
        ("I-IV-V-I", false, vec![(0, Major), (5, Major), (7, Major), (0, Major)]),
        ("I-vi-IV-V", false, vec![(0, Major), (9, Minor), (5, Major), (7, Major)]),
        ("I-V-vi-IV", false, vec![(0, Major), (7, Major), (9, Minor), (5, Major)]),
        ("ii-V-I-vi", false, vec![(2, Minor), (7, Major), (0, Major), (9, Minor)]),
        ("I-iii-IV-V", false, vec![(0, Major), (4, Minor), (5, Major), (7, Major)]),
        ("i-iv-V-i", true, vec![(0, Minor), (5, Minor), (7, Major), (0, Minor)]),
        ("i-VI-III-VII", true, vec![(0, Minor), (8, Major), (3, Major), (10, Major)]),
    ]
}

/// Progressions whose truth has seventh chords: the melody is built from all four notes.
fn seventh_progressions() -> Vec<(&'static str, Vec<(i32, Quality)>)> {
    use Quality::*;
    vec![
        ("ii7-V7-I-vi", vec![(2, Minor7), (7, Dominant7), (0, Major), (9, Minor)]),
        ("I-IV-V7-I", vec![(0, Major), (5, Major), (7, Dominant7), (0, Major)]),
        ("I-vi7-ii7-V7", vec![(0, Major), (9, Minor7), (2, Minor7), (7, Dominant7)]),
    ]
}

fn tones(q: Quality) -> Vec<i32> {
    match q {
        Quality::Minor => vec![0, 3, 7],
        Quality::Dominant7 => vec![0, 4, 7, 10],
        Quality::Minor7 => vec![0, 3, 7, 10],
        Quality::Major7 => vec![0, 4, 7, 11],
        _ => vec![0, 4, 7],
    }
}

fn melody(chords: &[(i32, Quality)], tonic: i32, rng: &mut Lcg) -> Score {
    let mut s = Score::new("eval", TimeSig { num: 4, den: 4 }, 0, chords.len());
    let mut prev = 72;
    for (m, &(root, q)) in chords.iter().enumerate() {
        let cts: Vec<i32> = (0..3).flat_map(|o| tones(q).into_iter().map(move |t| 60 + o * 12 + t).collect::<Vec<_>>()).map(|x| x + (tonic + root).rem_euclid(12)).collect();
        let nearest = |from: i32, set: &[i32]| *set.iter().min_by_key(|&&x| (x - from).abs()).unwrap();
        for beat in 0..4 {
            let strong = beat % 2 == 0;
            let note = if strong || rng.next(2) == 0 {
                // a chord tone near the previous note
                let c = nearest(prev + rng.next(5) as i32 - 2, &cts);
                c.clamp(60, 84)
            } else {
                // a scale neighbour (may or may not be a chord tone)
                let step = if rng.next(2) == 0 { 1 } else { -1 };
                let scale: Vec<i32> = (48..96i32).filter(|x| MAJOR.contains(&x.rem_euclid(12))).collect();
                let i = scale.iter().position(|&x| x >= prev).unwrap_or(0) as i32;
                scale[(i + step).clamp(0, scale.len() as i32 - 1) as usize]
            };
            s.put_abs(0, m as u32 * 1920 + beat as u32 * 480, 480, &[Pitch::from_midi(note, true)]).unwrap();
            prev = note;
        }
    }
    s
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        synthetic();
        sevenths_truth();
    } else {
        for f in files {
            real(&f);
        }
    }
}

fn synthetic() {
    let variants: [(&str, HarmonyOptions); 4] = [
        ("triadas", HarmonyOptions::default()),
        ("séptimas", HarmonyOptions { sevenths: true, ..HarmonyOptions::default() }),
        ("inversiones", HarmonyOptions { inversions: true, ..HarmonyOptions::default() }),
        ("secundarias", HarmonyOptions { secondary: true, ..HarmonyOptions::default() }),
    ];
    for (name, opts) in variants {
        let (mut bars, mut root_ok, mut exact_ok) = (0, 0, 0);
        let mut per_prog = Vec::new();
        for (pname, minor, prog) in progressions() {
            let (mut n, mut r, mut e) = (0, 0, 0);
            for seed in 0..20u64 {
                let mut rng = Lcg(seed * 7919 + 13);
                // the progression twice, so it is 8 bars long and ends on a tonic-ish chord
                let tonic = if minor { 9 } else { 0 };
                let chords: Vec<(i32, Quality)> = prog.iter().chain(prog.iter()).copied().collect();
                // the melody is written over the real chords (roots relative to the key signature of C)
                let abs: Vec<(i32, Quality)> = chords.iter().map(|&(o, q)| ((tonic + o).rem_euclid(12), q)).collect();
                let mut s = melody(&abs.iter().map(|&(root, q)| (root, q)).collect::<Vec<_>>(), 0, &mut rng);
                if s.harmonize(0, minor, 1, opts).is_err() {
                    continue;
                }
                if std::env::var("NOVA_VERBOSE").is_ok() && seed < 2 && name != "inversiones" {
                    let nm = |r: i32, q: Quality| format!("{}{}", ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"][r.rem_euclid(12) as usize], q.suffix());
                    let truth: Vec<String> = abs.iter().map(|&(r, q)| nm(r, q)).collect();
                    let found: Vec<String> = (0..abs.len()).map(|m| s.chords.iter().filter(|c| c.measure <= m).last().map_or("-".into(), |c| nm(c.root as i32, c.quality))).collect();
                    println!("  [{name} {pname} #{seed}]
    verdad {}
    hallado {}", truth.join(" "), found.join(" "));
                }
                for (m, &(root, q)) in abs.iter().enumerate() {
                    let found = s.chords.iter().filter(|c| c.measure <= m).last();
                    n += 1;
                    if let Some(c) = found {
                        r += (c.root as i32 == root) as u32;
                        e += (c.root as i32 == root && c.quality.triad() == q.triad()) as u32;
                    }
                }
            }
            per_prog.push(format!("{pname} {:.0}%", 100.0 * e as f64 / n.max(1) as f64));
            bars += n;
            root_ok += r;
            exact_ok += e;
        }
        println!("{name:<12} compases {bars}: fundamental {:.1} %, tríada exacta {:.1} %   [{}]", 100.0 * root_ok as f64 / bars as f64, 100.0 * exact_ok as f64 / bars as f64, per_prog.join(", "));
    }
}

/// Truth with seventh chords: how often the root is found, and how often the dominant seventh is written as one.
fn sevenths_truth() {
    for (name, opts) in [("triadas", HarmonyOptions::default()), ("séptimas", HarmonyOptions { sevenths: true, ..HarmonyOptions::default() })] {
        let (mut n, mut root_ok, mut dom_n, mut dom_ok, mut false7) = (0, 0, 0, 0, 0);
        for (_, prog) in seventh_progressions() {
            for seed in 0..20u64 {
                let mut rng = Lcg(seed * 104729 + 7);
                let chords: Vec<(i32, Quality)> = prog.iter().chain(prog.iter()).copied().collect();
                let mut s = melody(&chords, 0, &mut rng);
                if s.harmonize(0, false, 1, opts).is_err() {
                    continue;
                }
                for (m, &(root, q)) in chords.iter().enumerate() {
                    let Some(c) = s.chords.iter().filter(|c| c.measure <= m).last() else { continue };
                    n += 1;
                    root_ok += (c.root as i32 == root) as u32;
                    if q == Quality::Dominant7 {
                        dom_n += 1;
                        dom_ok += (c.root as i32 == root && c.quality == Quality::Dominant7) as u32;
                    } else {
                        false7 += matches!(c.quality, Quality::Dominant7 | Quality::Major7 | Quality::Minor7) as u32;
                    }
                }
            }
        }
        println!("verdad con 7ª, {name:<9} compases {n}: fundamental {:.1} %, V7 escrito como 7ª {:.1} % ({dom_ok}/{dom_n}), séptimas puestas donde no había V7: {false7}", 100.0 * root_ok as f64 / n as f64, 100.0 * dom_ok as f64 / dom_n.max(1) as f64);
    }
}

fn real(path: &str) {
    let bytes = std::fs::read(path).expect("cannot read");
    let r = if path.ends_with(".mscz") || path.ends_with(".mscx") { nova_core::mscx::import_mscz(&bytes) } else { nova_core::io::import_musicxml_bytes(&bytes) };
    let mut s = match r {
        Ok(r) => r.score,
        Err(e) => return println!("{path}: {e}"),
    };
    let part = (0..s.parts.len()).find(|&i| !s.parts[i].percussion).unwrap_or(0);
    s.chords.clear();
    let t = std::time::Instant::now();
    let opts = HarmonyOptions { sevenths: true, inversions: true, secondary: true };
    match s.harmonize(part, false, 2, opts) {
        Ok(n) => {
            let names: Vec<String> = s.chords.iter().take(40).map(|c| c.name(s.fifths)).collect();
            let measures = s.parts[part].measures.len();
            let (dom7, inv) = (s.chords.iter().filter(|c| c.quality == Quality::Dominant7).count(), s.chords.iter().filter(|c| c.bass != c.root).count());
            println!("{path}: {measures} compases, {n} acordes en {:?} (7ª de dominante: {dom7}, invertidos: {inv})\n   {}", t.elapsed(), names.join(" "));
        }
        Err(e) => println!("{path}: {e}"),
    }
}
