import { drumPos } from "./drum";
import { EventData, measureLen, notatedTicks, partKeyAt, PartKey, Pitch, pitchMidi, Score, timeAt, TimeSig, Transpose, voiceCount, voiceEvents } from "./types";

export const SP = 10; // staff space in px
export const STAFF_H = SP * 4;
const PAGE_W = 1000;
const MARGIN_X = 30;
const TOP = 110;
const PART_GAP = 56;
const SYSTEM_GAP = 70;
/** Extra space between systems when chord symbols are drawn above the first staff. */
const CHORD_GAP = 30;
/** Height of one verse line of lyrics, and the approximate width of one lyric character. */
export const LYRIC_LINE = 14;
export const LYRIC_CHAR_W = 6.2;
/** Extra distance of lyrics and dynamics below a tablature-only part (its stems and tuplet numbers take the room under the staff). */
export const TAB_LYRIC_DY = 10;

/** Width a lyric syllable needs, with room for the hyphen that follows it. */
export const lyricWidth = (text: string, syllabic: string) => text.length * LYRIC_CHAR_W + (syllabic === "begin" || syllabic === "middle" ? 8 : 4);

export interface LAccidental {
  pitchIndex: number; // index in ev.pitches
  alter: number;
  col: number;
}

export interface LEvent {
  ev: EventData;
  voice: number;
  x: number; // notehead left edge
  slotX: number;
  slotW: number;
  accs: LAccidental[];
  accCols: number;
  stemUp: boolean;
}

export interface LBeam {
  from: number; // inclusive indices into the lane's event list
  to: number;
  stemUp: boolean;
}

/** One voice of one staff inside a measure. */
export interface LLane {
  part: number;
  voice: number;
  /** True when the staff has more than one voice in this measure (stems/rests are split). */
  multi: boolean;
  events: LEvent[];
  beams: LBeam[];
}

export interface LMeasure {
  index: number;
  x: number;
  width: number;
  firstInSystem: boolean;
  showTime: boolean;
  /** A key change is drawn at the start of the measure (not for the first measure of a system, which has the full key signature). */
  showKey: boolean;
  /** More than one measure of rests in a row drawn as one block: how many (the measure then has no lanes). */
  restSpan?: number;
  /** Time signature in force (drawn when `showTime`). */
  time: TimeSig;
  headerW: number;
  lanes: LLane[];
}

export interface LSystem {
  top: number; // y of first part's top staff line
  measures: LMeasure[];
}

export interface LayoutResult {
  width: number;
  height: number;
  systems: LSystem[];
  staffTops: (system: LSystem) => number[];
  /** Distance from the top of a staff to the top of its tablature staff (parts with a tuning). */
  tabDy: number;
}

/** Line spacing of a tablature staff and the height of the 6-line staff drawn under a part. */
export const TAB_SP = 8;
export const tabHeight = (lines: number) => (lines - 1) * TAB_SP;

const SHARP_ORDER = [3, 0, 4, 1, 5, 2, 6]; // F C G D A E B
const FLAT_ORDER = [6, 2, 5, 1, 4, 0, 3]; // B E A D G C F

/** Naturals needed to cancel the key `prev` when going to `next`: all of them if the sign changes, else the surplus. */
export function cancelledSigns(prev: number, next: number): number {
  if (!prev) return 0;
  return Math.sign(prev) !== Math.sign(next) ? Math.abs(prev) : Math.max(0, Math.abs(prev) - Math.abs(next));
}

export function keyAlter(fifths: number, step: number): number {
  if (fifths > 0) return SHARP_ORDER.slice(0, fifths).includes(step) ? 1 : 0;
  if (fifths < 0) return FLAT_ORDER.slice(0, -fifths).includes(step) ? -1 : 0;
  return 0;
}

/** Diatonic index of the bottom staff line for each clef (E4 treble, G2 bass). */
export const bottomLineIndex = (clef: "treble" | "bass") => (clef === "treble" ? 30 : 18);

export const diatonic = (p: Pitch) => p.octave * 7 + p.step;

/** Index on the staff (same scale as `bottomLineIndex`): drum notes use a fixed position table. */
export const staffIdx = (percussion: boolean, p: Pitch) => (percussion ? bottomLineIndex("treble") + drumPos(pitchMidi(p)) : diatonic(p));

/** Split a (possibly dotted) duration into base value and dot flag. */
export function baseOf(ticks: number): { base: number; dotted: boolean } {
  let v = 1920;
  while (v >= 30) {
    if (ticks === v) return { base: v, dotted: false };
    if (ticks === v + v / 2) return { base: v, dotted: true };
    v /= 2;
  }
  return { base: ticks, dotted: false };
}

const slotWeight = (ticks: number) => 22 + 34 * Math.sqrt(ticks / 480);

/** Ticks per beaming group for the time signature. */
function beamSpan(num: number, den: number): number {
  if (den === 8) return num === 3 ? 1440 : 720;
  return 1920 / den;
}

const midOf = (ev: EventData, percussion = false) => {
  if (ev.kind !== "note") return null;
  const ids = ev.pitches.map((p) => staffIdx(percussion, p));
  return (Math.min(...ids) + Math.max(...ids)) / 2;
};

/**
 * Where a higher voice has a head on the same line or space as a head of a lower voice that is
 * sounding at the same time, move the higher voice sideways so both stay readable.
 */
function applyVoiceOffsets(lanes: LLane[], percussion: boolean[]) {
  const HEAD = 12;
  for (const lane of lanes) {
    if (lane.voice === 0) continue;
    for (const le of lane.events) {
      if (le.ev.kind !== "note") continue;
      const mine = le.ev.pitches.map((p) => staffIdx(percussion[lane.part], p));
      const clash = lanes.some(
        (o) =>
          o.part === lane.part &&
          o.voice < lane.voice &&
          o.events.some((oe) => {
            if (oe.ev.kind !== "note") return false;
            const sounding = oe.ev.start <= le.ev.start && le.ev.start < oe.ev.start + oe.ev.ticks;
            return sounding && oe.ev.pitches.some((q) => mine.some((a) => Math.abs(a - staffIdx(percussion[lane.part], q)) <= 1));
          }),
      );
      if (clash) le.x += HEAD;
    }
  }
}

/** Options of `layoutScore`. */
export interface LayoutOptions {
  /** Draw runs of two or more measures of rests as one block with a number (as parts are printed). */
  multiRests?: boolean;
}

export function layoutScore(score: Score, pageWidth = PAGE_W, opts: LayoutOptions = {}): LayoutResult {
  const nMeasures = Math.max(...score.parts.map((p) => p.measures.length));
  // Transposing staves read their own key; the header is as wide as the widest one.
  const partKey = (p: { transpose?: Transpose; key?: PartKey }, m: number) => partKeyAt(score, p, m);
  const keyBlockW = (n: number) => (n ? n * 11 + 8 : 0);
  const sysKeyW = (idx: number) => keyBlockW(Math.max(...score.parts.map((p) => Math.abs(partKey(p, idx)))));
  /** Signs drawn where the key changes: naturals for what is cancelled, then the new signs. */
  const changeSigns = (idx: number, p: { transpose?: Transpose; key?: PartKey }) => (partKey(p, idx) === partKey(p, idx - 1) ? 0 : cancelledSigns(partKey(p, idx - 1), partKey(p, idx)) + Math.abs(partKey(p, idx)));
  /** The key changes at this measure in the score or in any part with a key of its own. */
  const isKeyChange = (idx: number) => idx > 0 && score.parts.some((p) => partKey(p, idx) !== partKey(p, idx - 1));
  const changeW = (idx: number) => (isKeyChange(idx) ? keyBlockW(Math.max(...score.parts.map((p) => changeSigns(idx, p)))) : 0);
  const clefW = 42;
  const timeW = 30;
  const hasTime = (idx: number) => idx === 0 || (score.time_changes ?? []).some((c) => c.measure === idx);

  interface Prepared {
    index: number;
    onsets: number[];
    slotW: number[];
    accCols: number[];
    lanes: LLane[];
    contentW: number;
    time: TimeSig;
    restSpan?: number;
  }

  const prepared: Prepared[] = [];
  for (let m = 0; m < nMeasures; m++) {
    const mlen = measureLen(score, m);
    const span = beamSpan(timeAt(score, m).num, timeAt(score, m).den);
    const lanes: LLane[] = [];

    score.parts.forEach((p, pi) => {
      const meas = p.measures[m];
      const bottom = bottomLineIndex(p.clef);
      const nv = meas ? voiceCount(meas) : 1;
      const multi = nv > 1;
      const partLanes: LLane[] = [];
      for (let v = 0; v < nv; v++) {
        const evs = meas ? voiceEvents(meas, v) : null;
        if (!evs && v > 0) continue;
        const events: LEvent[] = (evs ?? []).map((ev) => {
          const mid = midOf(ev, p.percussion);
          const stemUp = multi ? v % 2 === 0 : mid !== null ? mid - bottom < 4 : false;
          return { ev, voice: v, x: 0, slotX: 0, slotW: 0, accs: [], accCols: 0, stemUp };
        });
        partLanes.push({ part: pi, voice: v, multi, events, beams: [] });
      }

      // Accidentals: one running state per staff and measure, in time order across voices.
      const all = partLanes.flatMap((l) => l.events).sort((a, b) => a.ev.start - b.ev.start || a.voice - b.voice);
      const state = new Map<number, number>(); // diatonic idx -> current alter
      for (const le of all) {
        const ev = le.ev;
        if (ev.kind !== "note" || p.percussion) continue;
        const accs: LAccidental[] = [];
        ev.pitches.forEach((pitch, i) => {
          const idx = diatonic(pitch);
          const cur = state.get(idx) ?? keyAlter(partKey(p, m), pitch.step);
          if (cur !== pitch.alter) accs.push({ pitchIndex: i, alter: pitch.alter, col: 0 });
          state.set(idx, pitch.alter);
        });
        // Stack accidentals of a chord from the top; a column is reused when >= a sixth apart.
        const order = [...accs].sort((a, b) => diatonic(ev.pitches[b.pitchIndex]) - diatonic(ev.pitches[a.pitchIndex]));
        const colPos: number[][] = [];
        for (const a of order) {
          const pos = diatonic(ev.pitches[a.pitchIndex]);
          let c = colPos.findIndex((list) => list.every((q) => Math.abs(q - pos) >= 6));
          if (c < 0) {
            colPos.push([]);
            c = colPos.length - 1;
          }
          colPos[c].push(pos);
          a.col = c;
        }
        le.accs = accs;
        le.accCols = colPos.length;
      }

      // Beam groups per lane: consecutive eighth-or-shorter notes inside the same beat group.
      for (const lane of partLanes) {
        const evs = lane.events;
        let startIdx = -1;
        const close = (end: number) => {
          if (startIdx >= 0 && end - startIdx >= 1) {
            let sum = 0;
            for (let k = startIdx; k <= end; k++) sum += (midOf(evs[k].ev, p.percussion) ?? 0) - bottom;
            const up = multi ? lane.voice % 2 === 0 : sum / (end - startIdx + 1) < 4;
            for (let k = startIdx; k <= end; k++) evs[k].stemUp = up;
            lane.beams.push({ from: startIdx, to: end, stemUp: up });
          }
          startIdx = -1;
        };
        evs.forEach((le, k) => {
          const eligible = le.ev.kind === "note" && baseOf(notatedTicks(le.ev)).base <= 240;
          if (!eligible) return close(k - 1);
          const same = startIdx >= 0 && Math.floor(evs[startIdx].ev.start / span) === Math.floor(le.ev.start / span);
          const contiguous = startIdx >= 0 && evs[k - 1].ev.start + evs[k - 1].ev.ticks === le.ev.start;
          if (startIdx >= 0 && !(same && contiguous)) close(k - 1);
          if (startIdx < 0) startIdx = k;
        });
        close(evs.length - 1);
      }
      lanes.push(...partLanes);
    });

    const onsetSet = new Set<number>();
    for (const l of lanes) l.events.forEach((le) => onsetSet.add(le.ev.start));
    const onsets = [...onsetSet].sort((a, b) => a - b);
    const accCols = onsets.map(() => 0);
    for (const l of lanes) for (const le of l.events) {
      const oi = onsets.indexOf(le.ev.start);
      accCols[oi] = Math.max(accCols[oi], le.accCols);
    }
    // A syllable must fit under its note: widen the slot when the text is longer than the note.
    const lyricW = onsets.map(() => 0);
    for (const l of lanes) for (const le of l.events) {
      const w = Math.max(0, ...(le.ev.lyrics ?? []).map((y) => lyricWidth(y.text, y.syllabic)));
      const oi = onsets.indexOf(le.ev.start);
      lyricW[oi] = Math.max(lyricW[oi], w);
    }
    const slotW = onsets.map((o, i) => {
      const next = i + 1 < onsets.length ? onsets[i + 1] : mlen;
      return Math.max(slotWeight(next - o) + (accCols[i] ? 3 + 10 * accCols[i] : 0), lyricW[i]);
    });
    prepared.push({ index: m, onsets, slotW, accCols, lanes, contentW: slotW.reduce((a, b) => a + b, 0), time: timeAt(score, m) });
  }

  // Runs of empty measures become one block: only measures that are rests in every part, with nothing written in them
  // (no text, dynamics, lyrics, articulations, repeats, markers, chord symbols or spanners) and no signature change inside.
  const plainRest = (m: number) =>
    score.parts.every((p) => {
      const me = p.measures[m];
      if (!me) return true;
      if (me.start_repeat || me.end_repeat || me.markers?.length || me.jump) return false;
      return [me.events, ...(me.voices ?? [])].every((evs) => evs.every((e) => e.kind === "rest" && !e.text && !e.dynamic && !e.lyrics?.length && !e.articulations?.length));
    }) &&
    !(score.chords ?? []).some((c) => c.measure === m) &&
    !score.parts.some((p) => (p.spanners ?? []).some((sp) => sp.start.measure <= m && m <= sp.end.measure));
  const REST_BLOCK_W = 90;
  const blocks: Prepared[] = [];
  for (let i = 0; i < prepared.length; i++) {
    let j = i;
    if (opts.multiRests && plainRest(prepared[i].index)) {
      while (j + 1 < prepared.length && plainRest(prepared[j + 1].index) && !hasTime(prepared[j + 1].index) && !isKeyChange(prepared[j + 1].index)) j++;
    }
    blocks.push(j > i ? { ...prepared[i], onsets: [0], slotW: [REST_BLOCK_W], accCols: [0], lanes: [], contentW: REST_BLOCK_W, restSpan: j - i + 1 } : prepared[i]);
    i = j;
  }

  // Greedy line breaking.
  const usable = pageWidth - 2 * MARGIN_X;
  const lines: Prepared[][] = [];
  let cur: Prepared[] = [];
  let used = 0;
  const headerFor = (first: boolean, idx: number) => (first ? clefW + sysKeyW(idx) : changeW(idx)) + (hasTime(idx) ? timeW : 0);
  // A repeat sign at the start of a measure needs room before the first note.
  const extraL = (idx: number) => (score.parts[0].measures[idx]?.start_repeat ? 12 : 0);
  for (const p of blocks) {
    const w = p.contentW + 22 + extraL(p.index) + headerFor(cur.length === 0, p.index);
    if (cur.length > 0 && used + w > usable) {
      lines.push(cur);
      cur = [];
      used = 0;
    }
    used += p.contentW + 22 + extraL(p.index) + headerFor(cur.length === 0, p.index);
    cur.push(p);
  }
  if (cur.length) lines.push(cur);

  const nParts = score.parts.length;
  // Lyrics hang below each staff, one line per verse, so the gap grows with the verses in use.
  const verses = score.parts.reduce((a, p) => Math.max(a, ...p.measures.flatMap((m) => [m.events, ...(m.voices ?? [])]).flatMap((evs) => evs.flatMap((e) => (e.lyrics ?? []).map((y) => y.verse + 1)))), 0);
  const lyricGap = verses ? verses * LYRIC_LINE + 4 + (score.parts.some((p) => p.tab_only) ? TAB_LYRIC_DY : 0) : 0;
  const partGap = PART_GAP + lyricGap;
  // A part with a tuning also holds a tablature staff under its staff.
  const tabDy = STAFF_H + 56 + lyricGap;
  const extra = (p: { tuning?: number[]; percussion?: boolean; tab_only?: boolean }) => (p.tuning && !p.percussion && !p.tab_only ? tabDy - STAFF_H + tabHeight(p.tuning.length) : 0);
  const systemH = score.parts.reduce((a, p) => a + STAFF_H + extra(p), 0) + (nParts - 1) * partGap;
  const systems: LSystem[] = [];
  const sysGap = SYSTEM_GAP + (score.chords?.length ? CHORD_GAP : 0) + lyricGap;
  lines.forEach((line, li) => {
    const top = TOP + li * (systemH + sysGap);
    const headers = line.map((p, i) => headerFor(i === 0, p.index));
    const fixed = headers.reduce((a, b) => a + b, 0) + 22 * line.length + line.reduce((a, p) => a + extraL(p.index), 0);
    const flex = line.reduce((a, p) => a + p.contentW, 0);
    let factor = (usable - fixed) / flex;
    if (li === lines.length - 1) factor = Math.min(factor, 1.35);
    let x = MARGIN_X;
    const measures: LMeasure[] = line.map((p, i) => {
      const width = headers[i] + 22 + extraL(p.index) + p.contentW * factor;
      let sx = x + headers[i] + 11 + extraL(p.index);
      const slots = new Map<number, [number, number, number]>();
      p.onsets.forEach((o, k) => {
        const sw = p.slotW[k] * factor;
        slots.set(o, [sx, sw, k]);
        sx += sw;
      });
      const lanes = p.lanes.map((lane) => ({
        ...lane,
        events: lane.events.map((le) => {
          const [sxx, sw, k] = slots.get(le.ev.start)!;
          const pad = p.accCols[k] ? 3 + 10 * p.accCols[k] : 4;
          return { ...le, slotX: sxx, slotW: sw, x: sxx + pad };
        }),
      }));
      applyVoiceOffsets(lanes, score.parts.map((q) => q.percussion));
      const lm: LMeasure = {
        index: p.index,
        x,
        width,
        firstInSystem: i === 0,
        showTime: hasTime(p.index),
        showKey: i > 0 && isKeyChange(p.index),
        restSpan: p.restSpan,
        time: p.time,
        headerW: headers[i],
        lanes,
      };
      x += width;
      return lm;
    });
    systems.push({ top, measures });
  });

  const height = TOP + lines.length * (systemH + sysGap);
  return {
    width: pageWidth,
    height,
    systems,
    staffTops: (s) => {
      let y = s.top;
      return score.parts.map((p) => {
        const top = y;
        y += STAFF_H + extra(p) + partGap;
        return top;
      });
    },
    tabDy,
  };
}
