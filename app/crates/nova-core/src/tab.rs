//! Tablature: which string and fret play each note of a part.
//!
//! Notes are stored at sounding pitch, which is what a guitar tab shows, so the fret of a pitch on a
//! string is simply `midi - open string`. The player can pin a string per note (`Event::strings`); the
//! rest is chosen here, keeping the hand near its previous position and chords inside a short span.
//! `src/tab.ts` repeats the algorithm for drawing: keep both in step.

use crate::{Content, Score};
use std::collections::HashMap;

/// Frets available on every string.
pub const MAX_FRET: i32 = 24;

/// Open strings (sounding MIDI pitch, lowest first) of the tunings offered by the interface.
pub const GUITAR: [u8; 6] = [40, 45, 50, 55, 59, 64];
pub const BASS: [u8; 4] = [28, 33, 38, 43];
/// Re-entrant ukulele tuning: G4 C4 E4 A4, listed as the lowest pitch first would break the string order, so the
/// strings are given from the lowest string number (the fourth, G4) to the first (A4).
pub const UKULELE: [u8; 4] = [67, 60, 64, 69];

/// A note on the neck: `string` counts from the highest string (1), as MusicXML does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fret {
    pub string: u8,
    pub fret: u8,
}

type Pick = Option<(usize, i32)>;

/// Where each pitch of a chord goes, one entry per pitch in the order given (`None` = out of range).
/// `manual[i]` (1-based from the highest string, 0 = automatic) pins a pitch to a string. `hand` is the
/// fret the hand rests on: it is read and updated, so successive calls follow the player's position.
pub fn assign(tuning: &[u8], pitches: &[i32], manual: &[u8], hand: &mut f64) -> Vec<Option<Fret>> {
    let n = tuning.len();
    if n == 0 {
        return vec![None; pitches.len()];
    }
    // candidates per pitch: (string index from the first listed string = 0.., fret)
    let cands: Vec<Vec<(usize, i32)>> = pitches
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let pinned = manual.get(i).copied().filter(|&m| m >= 1 && (m as usize) <= n);
            (0..n)
                .filter(|&s| pinned.map_or(true, |m| s == n - m as usize))
                .map(|s| (s, p - tuning[s] as i32))
                .filter(|&(_, f)| (0..=MAX_FRET).contains(&f))
                .collect()
        })
        .collect();
    let mut best: Option<(f64, Vec<Pick>)> = None;
    let mut choice: Vec<Pick> = vec![None; pitches.len()];
    let mut used = vec![false; n];

    // exhaustive search over the strings of a chord (at most 6 notes on 6 strings)
    fn walk(i: usize, cands: &[Vec<(usize, i32)>], used: &mut [bool], choice: &mut Vec<Pick>, hand: f64, best: &mut Option<(f64, Vec<Pick>)>) {
        if i == cands.len() {
            let fretted: Vec<i32> = choice.iter().flatten().map(|c| c.1).filter(|&f| f > 0).collect();
            let placed = choice.iter().flatten().count();
            let dropped = (cands.len() - placed) as f64;
            let span = match (fretted.iter().max(), fretted.iter().min()) {
                (Some(a), Some(b)) => (a - b) as f64,
                _ => 0.0,
            };
            let centre = if fretted.is_empty() { hand } else { fretted.iter().sum::<i32>() as f64 / fretted.len() as f64 };
            let high: f64 = fretted.iter().map(|&f| (f - 12).max(0) as f64).sum();
            let cost = 100.0 * dropped + 3.0 * span + (centre - hand).abs() + 0.15 * centre + 0.5 * high;
            if best.as_ref().map_or(true, |b| cost < b.0 - 1e-9) {
                *best = Some((cost, choice.clone()));
            }
            return;
        }
        let mut any = false;
        for &(s, f) in &cands[i] {
            if used[s] {
                continue;
            }
            any = true;
            used[s] = true;
            choice[i] = Some((s, f));
            walk(i + 1, cands, used, choice, hand, best);
            used[s] = false;
        }
        if !any {
            choice[i] = None;
            walk(i + 1, cands, used, choice, hand, best);
        }
        choice[i] = None;
    }
    walk(0, &cands, &mut used, &mut choice, *hand, &mut best);
    let picked = best.map(|b| b.1).unwrap_or_else(|| vec![None; pitches.len()]);
    let fretted: Vec<i32> = picked.iter().flatten().map(|c| c.1).filter(|&f| f > 0).collect();
    if !fretted.is_empty() {
        *hand = fretted.iter().sum::<i32>() as f64 / fretted.len() as f64;
    }
    picked.into_iter().map(|c| c.map(|(s, f)| Fret { string: (n - s) as u8, fret: f as u8 })).collect()
}

impl Score {
    /// Strings and frets of every note of `part` in `voice`, keyed by `(measure, start)`. Each value follows
    /// the ascending order of the event's pitches. Empty when the part has no tuning.
    pub fn tab_positions(&self, part: usize, voice: usize) -> HashMap<(usize, u32), Vec<Option<Fret>>> {
        let mut out = HashMap::new();
        let Some(p) = self.parts.get(part) else { return out };
        let Some(tuning) = p.tuning.as_deref() else { return out };
        let mut hand = 3.0;
        // the previous note when it is tied forward: (pitch, string) pairs a tied note must keep
        let mut tied: Vec<(i32, u8)> = Vec::new();
        for (mi, m) in p.measures.iter().enumerate() {
            for e in m.voice(voice).into_iter().flatten() {
                if let Content::Note { pitches, tie } = &e.content {
                    let midis: Vec<i32> = pitches.iter().map(|q| q.midi()).collect();
                    let mut manual: Vec<u8> = if e.strings.len() == midis.len() { e.strings.clone() } else { vec![0; midis.len()] };
                    for (k, m) in midis.iter().enumerate() {
                        if manual[k] == 0 {
                            if let Some(&(_, s)) = tied.iter().find(|(tm, _)| tm == m) {
                                manual[k] = s;
                            }
                        }
                    }
                    let frets = assign(tuning, &midis, &manual, &mut hand);
                    tied = if *tie { midis.iter().zip(&frets).filter_map(|(&mid, f)| f.map(|f| (mid, f.string))).collect() } else { Vec::new() };
                    out.insert((mi, e.start), frets);
                } else {
                    tied.clear();
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_strings_and_simple_frets() {
        let mut hand = 3.0;
        // E2 is the open low string (string 6), A2 open string 5, E4 open string 1
        assert_eq!(assign(&GUITAR, &[40], &[], &mut hand), vec![Some(Fret { string: 6, fret: 0 })]);
        assert_eq!(assign(&GUITAR, &[45], &[], &mut hand), vec![Some(Fret { string: 5, fret: 0 })]);
        assert_eq!(assign(&GUITAR, &[64], &[], &mut hand), vec![Some(Fret { string: 1, fret: 0 })]);
        // below the lowest string and above the neck: no position
        assert_eq!(assign(&GUITAR, &[39], &[], &mut hand), vec![None]);
        assert_eq!(assign(&GUITAR, &[64 + 25], &[], &mut hand), vec![None]);
    }

    #[test]
    fn a_pinned_string_is_respected_and_a_chord_uses_distinct_strings() {
        let mut hand = 3.0;
        // E3 (52): automatically the second fret of the D string
        assert_eq!(assign(&GUITAR, &[52], &[], &mut hand)[0], Some(Fret { string: 4, fret: 2 }));
        let mut hand = 3.0;
        // pinned to the A string (5): fret 7; pinned to the high E (1): out of reach
        assert_eq!(assign(&GUITAR, &[52], &[5], &mut hand), vec![Some(Fret { string: 5, fret: 7 })]);
        assert_eq!(assign(&GUITAR, &[52], &[1], &mut hand), vec![None]);
        // an open C chord: C3 E3 G3 C4 E4 on distinct strings inside the first frets
        let mut hand = 3.0;
        let c = assign(&GUITAR, &[48, 52, 55, 60, 64], &[], &mut hand);
        let mut strings: Vec<u8> = c.iter().map(|x| x.expect("playable").string).collect();
        strings.sort();
        strings.dedup();
        assert_eq!(strings.len(), 5, "{c:?}");
        assert!(c.iter().all(|x| x.unwrap().fret <= 3), "{c:?}");
    }

    #[test]
    fn the_hand_follows_a_scale_up_the_neck() {
        let mut hand = 3.0;
        let mut last = 0i32;
        let mut maxjump = 0;
        for midi in 40..=76 {
            let f = assign(&GUITAR, &[midi], &[], &mut hand)[0].expect("playable");
            let open = GUITAR[(6 - f.string) as usize] as i32;
            assert_eq!(open + f.fret as i32, midi);
            if f.fret > 0 {
                if last > 0 {
                    maxjump = maxjump.max((f.fret as i32 - last).abs());
                }
                last = f.fret as i32;
            }
        }
        assert!(maxjump <= 6, "{maxjump}");
    }

    #[test]
    fn a_tied_note_stays_on_the_string_of_the_note_it_continues() {
        use crate::{Pitch, Score, TimeSig};
        let p = |m: i32| Pitch::from_midi(m, true);
        let mut s = Score::new("t", TimeSig { num: 4, den: 4 }, 0, 2);
        s.parts[0].tuning = Some(GUITAR.to_vec());
        // E4 pinned to the D string (fret 14), tied across the barline into an E4 that would be the open first string alone
        s.put_note(0, 0, 0, 1440, 480, vec![p(64)]).unwrap();
        s.set_string(0, 0, 0, 1440, 64, 4).unwrap();
        s.put_note(0, 0, 1, 0, 480, vec![p(64)]).unwrap();
        let alone = s.tab_positions(0, 0)[&(1, 0)][0].unwrap();
        assert_eq!((alone.string, alone.fret), (1, 0), "without the tie the note is free to move");
        s.toggle_tie(0, 0, 0, 1440).unwrap();
        let tied = s.tab_positions(0, 0)[&(1, 0)][0].unwrap();
        assert_eq!((tied.string, tied.fret), (4, 14));
    }

    #[test]
    fn bass_and_ukulele_tunings() {
        let mut hand = 3.0;
        assert_eq!(assign(&BASS, &[28], &[], &mut hand), vec![Some(Fret { string: 4, fret: 0 })]);
        assert_eq!(assign(&BASS, &[43 + 5], &[], &mut hand), vec![Some(Fret { string: 1, fret: 5 })]);
        // the ukulele is re-entrant: the strings are listed 4th..1st (G4 C4 E4 A4)
        let mut hand = 3.0;
        assert_eq!(assign(&UKULELE, &[60], &[], &mut hand), vec![Some(Fret { string: 3, fret: 0 })]);
        assert_eq!(assign(&UKULELE, &[69], &[], &mut hand), vec![Some(Fret { string: 1, fret: 0 })]);
    }
}
