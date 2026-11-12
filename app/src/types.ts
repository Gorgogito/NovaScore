export const PPQ = 480;

export interface Pitch {
  step: number; // 0=C .. 6=B
  alter: number;
  octave: number;
}

export interface Tuplet {
  actual: number;
  normal: number;
  /** First tick of the group; identifies it inside the measure. */
  start: number;
}

export const DYNAMICS = ["ppp", "pp", "p", "mp", "mf", "f", "ff", "fff", "fp", "sf", "sfz", "rfz"] as const;
export type ArticKind = "staccato" | "staccatissimo" | "tenuto" | "accent" | "marcato" | "portato" | "mordent" | "shorttrill" | "turn" | "invertedturn";

export type Lyric = { verse: number; text: string; syllabic: "single" | "begin" | "middle" | "end"; extend_to?: { measure: number; tick: number } };
export type EventData = { start: number; ticks: number; tuplet?: Tuplet; dynamic?: string; articulations?: ArticKind[]; lyrics?: Lyric[]; text?: string; strings?: number[]; /** Set by `displayScore` on tablature parts: the sounding MIDI pitches (the drawn ones are shifted). */ snd?: number[] } & (
  | { kind: "note"; pitches: Pitch[]; tie: boolean }
  | { kind: "rest" }
);

export interface Measure {
  /** Voice 1. */
  events: EventData[];
  /** Voices 2-4; an empty list means the voice has no content in this measure. */
  voices?: EventData[][];
  start_repeat?: boolean;
  /** Times the section is played (0/undefined = no repeat sign at the end). */
  end_repeat?: number;
  /** Segno, coda, "To Coda" and fine marks of the measure. */
  markers?: Marker[];
  /** Jump taken at the end of the measure. */
  jump?: Jump;
}

export type Marker = "Segno" | "Coda" | "ToCoda" | "Fine";
export type Jump = "Dc" | "DcAlFine" | "DcAlCoda" | "Ds" | "DsAlFine" | "DsAlCoda";
export const JUMP_TEXT: Record<Jump, string> = {
  Dc: "D.C.",
  DcAlFine: "D.C. al Fine",
  DcAlCoda: "D.C. al Coda",
  Ds: "D.S.",
  DsAlFine: "D.S. al Fine",
  DsAlCoda: "D.S. al Coda",
};

export const MAX_VOICES = 4;

/** Written duration: tuplet events sound shorter than they are notated. */
export const notatedTicks = (e: { ticks: number; tuplet?: Tuplet }): number => {
  if (!e.tuplet) return e.ticks;
  const { actual: a, normal: n } = e.tuplet;
  if ((e.ticks * a) % n === 0) return (e.ticks * a) / n;
  // ratios that do not divide 480 evenly (7:4): the standard value that sounds within a tick of it
  for (let v = 1920; v >= 30; v /= 2) {
    for (const c of v / 2 >= 30 ? [v, v + v / 2] : [v]) if (Math.abs(c * n - e.ticks * a) <= a) return c;
  }
  return Math.round((e.ticks * a) / n);
};

/** Events of a voice (0-based) in a measure, or null if the voice is absent. */
export const voiceEvents = (m: Measure, v: number): EventData[] | null =>
  v === 0 ? m.events : m.voices?.[v - 1]?.length ? m.voices[v - 1] : null;

export const voiceCount = (m: Measure): number => {
  let last = -1;
  (m.voices ?? []).forEach((v, i) => {
    if (v.length) last = i;
  });
  return last + 2;
};

export type SpannerKind = "slur" | "crescendo" | "decrescendo" | "ottava8va" | "ottava8vb" | "ottava15ma" | "ottava15mb" | "pedal" | "trill" | "volta";

/** Octaves between sounding and written pitch under an octave line (8va = +1, 15mb = -2). */
export const ottavaOctaves = (k: SpannerKind): number => (k === "ottava8va" ? 1 : k === "ottava8vb" ? -1 : k === "ottava15ma" ? 2 : k === "ottava15mb" ? -2 : 0);

/** A point in time: measure and tick inside it (the tick may equal the measure length). */
export interface SpPos {
  measure: number;
  tick: number;
}

/** A slur between two notes of a voice, or a hairpin over a stretch of time. */
export interface Spanner {
  kind: SpannerKind;
  voice: number;
  start: SpPos;
  end: SpPos;
  /** Volta: bit n-1 set = played on pass n. */
  endings?: number;
}

/** A range of events of one voice, from `from` to `to` (inclusive). */
export interface Selection {
  part: number;
  voice: number;
  from: { m: number; start: number };
  to: { m: number; start: number };
}

export interface Part {
  name: string;
  clef: "treble" | "bass";
  program: number;
  volume: number;
  muted: boolean;
  solo: boolean;
  percussion: boolean;
  /** Transposing instrument: sounding = written + interval. Notes are stored at sounding pitch. */
  transpose?: Transpose;
  /** Tablature: open strings (sounding MIDI pitch) of the instrument; a tab staff is drawn under the staff. */
  tuning?: number[];
  /** Set by `displayScore`: key signature this staff is drawn with (written key of a transposing instrument). */
  shown_fifths?: number;
  spanners?: Spanner[];
  measures: Measure[];
}

export interface Transpose {
  diatonic: number;
  chromatic: number;
}

const SEMI = [0, 2, 4, 5, 7, 9, 11];
const SPELL_SHARP: [number, number][] = [[0, 0], [0, 1], [1, 0], [1, 1], [2, 0], [3, 0], [3, 1], [4, 0], [4, 1], [5, 0], [5, 1], [6, 0]];
const SPELL_FLAT: [number, number][] = [[0, 0], [1, -1], [1, 0], [2, -1], [2, 0], [3, 0], [4, -1], [4, 0], [5, -1], [5, 0], [6, -1], [6, 0]];

/** Move a pitch by `sign` times the interval (mirror of `Transpose::shift` in nova-core). */
const shiftPitch = (p: Pitch, t: Transpose, sign: number): Pitch => {
  const total = p.octave * 7 + p.step + sign * t.diatonic;
  const octave = Math.floor(total / 7);
  const step = ((total % 7) + 7) % 7;
  const target = pitchMidi(p) + sign * t.chromatic;
  const alter = target - ((octave + 1) * 12 + SEMI[step]);
  if (Math.abs(alter) > 2) {
    const [st, al] = (alter < 0 ? SPELL_FLAT : SPELL_SHARP)[((target % 12) + 12) % 12];
    return { step: st, alter: al, octave: Math.floor(target / 12) - 1 };
  }
  return { step, alter, octave };
};

/** Pitch as the player reads it, from the sounding one. */
export const writtenPitch = (p: Pitch, t?: Transpose): Pitch => (t ? shiftPitch(p, t, -1) : p);
/** Sounding pitch of a written one. */
export const soundingPitch = (p: Pitch, t?: Transpose): Pitch => (t ? shiftPitch(p, t, 1) : p);

/** Key signature a transposing instrument reads for a sounding major key. */
export const writtenFifths = (fifths: number, t?: Transpose): number => {
  if (!t) return fifths;
  const BASE = [0, 2, 4, -1, 1, 3, 5];
  const tonic: Pitch = { step: (((fifths * 4) % 7) + 7) % 7, alter: Math.floor((fifths + 1) / 7), octave: 4 };
  const w = writtenPitch(tonic, t);
  let out = BASE[w.step] + 7 * w.alter;
  while (out > 7) out -= 12;
  while (out < -7) out += 12;
  return out;
};

export interface TimeSig {
  num: number;
  den: number;
}

export interface Score {
  title: string;
  composer: string;
  /** Initial time signature. */
  time: TimeSig;
  /** Later changes (measure is 0-based); each applies until the next one. */
  time_changes?: { measure: number; time: TimeSig }[];
  fifths: number;
  tempo: number;
  parts: Part[];
  /** Chord symbols above the first staff; each lasts until the next one. */
  chords?: ChordMark[];
}

export interface ChordMark {
  measure: number;
  tick: number;
  /** Pitch class of the root (0 = C). */
  root: number;
  quality: "major" | "minor" | "dominant7" | "major7" | "minor7" | "dim" | "dim7";
  /** Pitch class of the bass (equal to `root` without inversion). */
  bass: number;
  /** Written by hand or read from a file: "Actualizar acordes" keeps it. */
  manual?: boolean;
}

const CHORD_SUFFIX: Record<ChordMark["quality"], string> = { major: "", minor: "m", dominant7: "7", major7: "maj7", minor7: "m7", dim: "dim", dim7: "dim7" };
const SHARP_NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const FLAT_NAMES = ["C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B"];

/** Chord symbol text (sharps or flats according to the key signature). */
/** Chord symbol text. Stored chords are at concert pitch; with `tr` (the first staff transposes) it is the written name. */
export const chordName = (c: ChordMark, fifths: number, tr?: Transpose): string => {
  const names = writtenFifths(fifths, tr) >= 0 ? SHARP_NAMES : FLAT_NAMES;
  const pc = (n: number) => (((n - (tr?.chromatic ?? 0)) % 12) + 12) % 12;
  return names[pc(c.root)] + CHORD_SUFFIX[c.quality] + (c.bass !== c.root ? "/" + names[pc(c.bass)] : "");
};

export interface Cursor {
  part: number;
  voice: number;
  m: number;
  start: number;
}

/** Time signature in force at measure `m`. */
export const timeAt = (s: Score, m: number): TimeSig => {
  let t = s.time;
  for (const c of s.time_changes ?? []) if (c.measure <= m) t = c.time;
  return t;
};

/** Octave shift in force for an event of a part (sum over the octave lines that cover it). */
export const ottavaShiftAt = (p: Part, measure: number, tick: number): number => {
  let k = 0;
  for (const sp of p.spanners ?? []) {
    const o = ottavaOctaves(sp.kind);
    if (o === 0) continue;
    const key = measure * 100000 + tick;
    if (key >= sp.start.measure * 100000 + sp.start.tick && key < sp.end.measure * 100000 + sp.end.tick) k += o;
  }
  return k;
};

/**
 * The score as it is drawn: notes under an octave line are written one or two octaves away from
 * where they sound. Editing keeps working on the real (sounding) score.
 */
export const displayScore = (s: Score): Score => {
  const hasOttava = (p: Part) => (p.spanners ?? []).some((x) => ottavaOctaves(x.kind) !== 0);
  if (!s.parts.some((p) => hasOttava(p) || p.transpose || p.tuning)) return s;
  return {
    ...s,
    parts: s.parts.map((p) => {
      if (!hasOttava(p) && !p.transpose && !p.tuning) return p;
      const shifted = (evs: EventData[], mi: number): EventData[] =>
        evs.map((e) => {
          if (e.kind !== "note") return e;
          const k = ottavaShiftAt(p, mi, e.start);
          const snd = p.tuning ? e.pitches.map(pitchMidi) : undefined;
          return { ...e, snd, pitches: e.pitches.map((q) => { const w = writtenPitch(q, p.transpose); return k ? { ...w, octave: w.octave - k } : w; }) };
        });
      return {
        ...p,
        shown_fifths: p.transpose ? writtenFifths(s.fifths, p.transpose) : undefined,
        measures: p.measures.map((m, mi) => ({ ...m, events: shifted(m.events, mi), voices: m.voices?.map((v) => shifted(v, mi)) })),
      };
    }),
  };
};

export const measureLen = (s: Score, m: number): number => {
  const t = timeAt(s, m);
  return (t.num * PPQ * 4) / t.den;
};

/** Absolute start tick of each of the first `n` measures. */
export const measureStarts = (s: Score, n: number): number[] => {
  const out: number[] = [];
  let pos = 0;
  for (let m = 0; m < n; m++) {
    out.push(pos);
    pos += measureLen(s, m);
  }
  return out;
};

export const pitchMidi = (p: Pitch) =>
  (p.octave + 1) * 12 + [0, 2, 4, 5, 7, 9, 11][p.step] + p.alter;

/** Notes under a melisma line: `m:start` -> [verse, is the last covered note]; `origins` holds `m:start:verse` of syllables with a line. */
export interface LyricCovers {
  covered: Map<string, [number, boolean][]>;
  origins: Set<string>;
}

const coverCache = new WeakMap<Score, Map<string, LyricCovers>>();

export const lyricCovers = (score: Score, part: number, voice: number): LyricCovers => {
  let perScore = coverCache.get(score);
  if (!perScore) coverCache.set(score, (perScore = new Map()));
  const key = `${part}:${voice}`;
  const hit = perScore.get(key);
  if (hit) return hit;
  const out: LyricCovers = { covered: new Map(), origins: new Set() };
  const all = score.parts[part].measures.flatMap((m, mi) => (voiceEvents(m, voice) ?? []).map((e) => ({ mi, e })));
  all.forEach(({ mi, e }, i) => {
    for (const l of e.lyrics ?? []) {
      const end = l.extend_to;
      if (!end) continue;
      let last: string | null = null;
      for (const { mi: m2, e: e2 } of all.slice(i + 1)) {
        if (m2 > end.measure || (m2 === end.measure && e2.start >= end.tick)) break;
        if (e2.kind !== "note") continue;
        last = `${m2}:${e2.start}`;
        out.covered.set(last, [...(out.covered.get(last) ?? []), [l.verse, false]]);
      }
      if (last) {
        out.origins.add(`${mi}:${e.start}:${l.verse}`);
        const list = out.covered.get(last)!;
        for (let k = list.length - 1; k >= 0; k--) {
          if (list[k][0] === l.verse) {
            list[k] = [l.verse, true];
            break;
          }
        }
      }
    }
  });
  perScore.set(key, out);
  return out;
};
