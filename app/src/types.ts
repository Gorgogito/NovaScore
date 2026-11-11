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
export type ArticKind = "staccato" | "staccatissimo" | "tenuto" | "accent" | "marcato" | "portato";

export type EventData = { start: number; ticks: number; tuplet?: Tuplet; dynamic?: string; articulations?: ArticKind[] } & (
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
}

export const MAX_VOICES = 4;

/** Written duration: tuplet events sound shorter than they are notated. */
export const notatedTicks = (e: { ticks: number; tuplet?: Tuplet }): number =>
  e.tuplet ? Math.round((e.ticks * e.tuplet.actual) / e.tuplet.normal) : e.ticks;

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
  spanners?: Spanner[];
  measures: Measure[];
}

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
}

const CHORD_SUFFIX: Record<ChordMark["quality"], string> = { major: "", minor: "m", dominant7: "7", major7: "maj7", minor7: "m7", dim: "dim", dim7: "dim7" };
const SHARP_NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const FLAT_NAMES = ["C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B"];

/** Chord symbol text (sharps or flats according to the key signature). */
export const chordName = (c: ChordMark, fifths: number): string => {
  const names = fifths >= 0 ? SHARP_NAMES : FLAT_NAMES;
  return names[c.root % 12] + CHORD_SUFFIX[c.quality] + (c.bass !== c.root ? "/" + names[c.bass % 12] : "");
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
  if (!s.parts.some((p) => (p.spanners ?? []).some((x) => ottavaOctaves(x.kind) !== 0))) return s;
  return {
    ...s,
    parts: s.parts.map((p) => {
      if (!(p.spanners ?? []).some((x) => ottavaOctaves(x.kind) !== 0)) return p;
      const shifted = (evs: EventData[], mi: number): EventData[] =>
        evs.map((e) => {
          if (e.kind !== "note") return e;
          const k = ottavaShiftAt(p, mi, e.start);
          return k ? { ...e, pitches: e.pitches.map((q) => ({ ...q, octave: q.octave - k })) } : e;
        });
      return { ...p, measures: p.measures.map((m, mi) => ({ ...m, events: shifted(m.events, mi), voices: m.voices?.map((v) => shifted(v, mi)) })) };
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
