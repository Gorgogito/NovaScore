// Tablature: which string and fret play each note. Port of `nova-core/src/tab.rs` (keep both in step):
// notes are at sounding pitch, so the fret of a pitch on a string is `midi - open string`.
import { Part, pitchMidi, Score, voiceEvents } from "./types";

export const MAX_FRET = 24;

/** Open strings (sounding MIDI pitch, listed from string n to string 1, i.e. the lowest string first for guitar and bass). */
export const TUNINGS: { name: string; strings: number[]; /** Written an octave above the sound (guitar and bass notation). */ octave: boolean }[] = [
  { name: "Guitarra (6 cuerdas)", strings: [40, 45, 50, 55, 59, 64], octave: true },
  { name: "Bajo (4 cuerdas)", strings: [28, 33, 38, 43], octave: true },
  { name: "Ukelele", strings: [67, 60, 64, 69], octave: false },
];

export const OCTAVE_BELOW = { diatonic: -7, chromatic: -12 };

export interface Fret {
  /** 1 = highest string (as in MusicXML). */
  string: number;
  fret: number;
}

type Pick = [number, number] | null; // [string index in the tuning list, fret]

/** Where each pitch of a chord goes (`null` = out of range). `hand` is the fret the hand rests on (read and updated). */
export const assignFrets = (tuning: number[], pitches: number[], manual: number[], hand: { v: number }): (Fret | null)[] => {
  const n = tuning.length;
  if (n === 0) return pitches.map(() => null);
  const cands: [number, number][][] = pitches.map((p, i) => {
    const m = manual[i];
    const pinned = m !== undefined && m >= 1 && m <= n ? m : 0;
    const out: [number, number][] = [];
    for (let s = 0; s < n; s++) {
      if (pinned && s !== n - pinned) continue;
      const f = p - tuning[s];
      if (f >= 0 && f <= MAX_FRET) out.push([s, f]);
    }
    return out;
  });
  let best: { cost: number; pick: Pick[] } | null = null;
  const choice: Pick[] = pitches.map(() => null);
  const used: boolean[] = tuning.map(() => false);
  const walk = (i: number) => {
    if (i === cands.length) {
      const fretted = choice.flatMap((c) => (c && c[1] > 0 ? [c[1]] : []));
      const placed = choice.filter((c) => c).length;
      const dropped = cands.length - placed;
      const span = fretted.length ? Math.max(...fretted) - Math.min(...fretted) : 0;
      const centre = fretted.length ? fretted.reduce((a, b) => a + b, 0) / fretted.length : hand.v;
      const high = fretted.reduce((a, f) => a + Math.max(f - 12, 0), 0);
      const cost = 100 * dropped + 3 * span + Math.abs(centre - hand.v) + 0.15 * centre + 0.5 * high;
      if (!best || cost < best.cost - 1e-9) best = { cost, pick: choice.map((c) => (c ? [c[0], c[1]] : null)) };
      return;
    }
    let any = false;
    for (const [s, f] of cands[i]) {
      if (used[s]) continue;
      any = true;
      used[s] = true;
      choice[i] = [s, f];
      walk(i + 1);
      used[s] = false;
    }
    if (!any) {
      choice[i] = null;
      walk(i + 1);
    }
    choice[i] = null;
  };
  walk(0);
  const picked: Pick[] = best ? (best as { pick: Pick[] }).pick : pitches.map(() => null);
  const fretted = picked.flatMap((c) => (c && c[1] > 0 ? [c[1]] : []));
  if (fretted.length) hand.v = fretted.reduce((a, b) => a + b, 0) / fretted.length;
  return picked.map((c) => (c ? { string: n - c[0], fret: c[1] } : null));
};

const cache = new WeakMap<Score, Map<string, Map<string, (Fret | null)[]>>>();

/** Strings and frets of the notes of a part's voice, keyed by `measure:start`; values follow the ascending pitches. */
export const tabPositions = (score: Score, part: number, voice: number): Map<string, (Fret | null)[]> => {
  let perScore = cache.get(score);
  if (!perScore) cache.set(score, (perScore = new Map()));
  const key = `${part}:${voice}`;
  const hit = perScore.get(key);
  if (hit) return hit;
  const out = new Map<string, (Fret | null)[]>();
  const p: Part | undefined = score.parts[part];
  const tuning = p?.tuning;
  if (tuning) {
    const hand = { v: 3 };
    // a note tied forward hands its strings to the tied pitches of the next note
    let tied: [number, number][] = [];
    p.measures.forEach((m, mi) => {
      for (const e of voiceEvents(m, voice) ?? []) {
        if (e.kind !== "note") {
          tied = [];
          continue;
        }
        const midis = e.snd ?? e.pitches.map(pitchMidi);
        const manual = midis.map((md, k) => (e.strings && e.strings.length === midis.length && e.strings[k]) || tied.find(([tm]) => tm === md)?.[1] || 0);
        const frets = assignFrets(tuning, midis, manual, hand);
        tied = e.tie ? midis.flatMap((md, k) => (frets[k] ? [[md, frets[k]!.string] as [number, number]] : [])) : [];
        out.set(`${mi}:${e.start}`, frets);
      }
    });
  }
  perScore.set(key, out);
  return out;
};

/** Strings (1 = highest) where a pitch can be played. */
export const playableStrings = (tuning: number[], midi: number): number[] => {
  const n = tuning.length;
  const out: number[] = [];
  for (let s = 0; s < n; s++) {
    const f = midi - tuning[s];
    if (f >= 0 && f <= MAX_FRET) out.push(n - s);
  }
  return out.sort((a, b) => a - b);
};
