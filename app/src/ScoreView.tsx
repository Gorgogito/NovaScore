import { createMemo, For, Show } from "solid-js";
import { DRUM_X } from "./drum";
import { baseOf, bottomLineIndex, cancelledSigns, LayoutResult, LBeam, LEvent, LLane, LMeasure, LSystem, LYRIC_CHAR_W, LYRIC_LINE, SP, TAB_LYRIC_DY, TAB_SP, tabHeight, staffIdx, STAFF_H } from "./layout";
import { tabPositions } from "./tab";
import { chordName, ChordMark, Cursor, EventData, JUMP_TEXT, keyAt, lyricCovers, measureLen, notatedTicks, partKeyAt, Pitch, PPQ, pitchMidi, Score, Selection, Spanner, SpPos, voiceEvents } from "./types";

const g = (cp: number) => String.fromCodePoint(cp);
const g2 = g;
const FONT = String(SP * 4);
const REST_GLYPH: Record<number, number> = { 1920: 0xe4e3, 960: 0xe4e4, 480: 0xe4e5, 240: 0xe4e6, 120: 0xe4e7, 60: 0xe4e8, 30: 0xe4e9 };
const ACC_GLYPH: Record<number, number> = { [-2]: 0xe264, [-1]: 0xe260, 0: 0xe261, 1: 0xe262, 2: 0xe263 };
const FLAG_UP = [0, 0xe240, 0xe242, 0xe244, 0xe246];
const FLAG_DOWN = [0, 0xe241, 0xe243, 0xe245, 0xe247];
const SHARP_POS = [8, 5, 9, 6, 3, 7, 4];
const FLAT_POS = [4, 7, 3, 6, 2, 5, 1];
const BEAM_T = 5;
// SMuFL dynamics are built from single letters: p m f r s z.
const DYN_GLYPH: Record<string, number> = { p: 0xe520, m: 0xe521, f: 0xe522, r: 0xe523, s: 0xe524, z: 0xe525 };
const dynamicText = (name: string) => [...name].map((c) => String.fromCodePoint(DYN_GLYPH[c] ?? 0x20)).join("");
// Articulation glyphs: [above, below] (SMuFL).
const ARTIC_GLYPH: Record<string, [number, number]> = {
  staccato: [0xe4a2, 0xe4a3],
  staccatissimo: [0xe4a6, 0xe4a7],
  tenuto: [0xe4a4, 0xe4a5],
  accent: [0xe4a0, 0xe4a1],
  marcato: [0xe4ac, 0xe4ad],
  portato: [0xe4b2, 0xe4b3],
};
// Ornaments are always drawn above the staff.
const ORNAMENT_GLYPH: Record<string, number> = { mordent: 0xe56d, shorttrill: 0xe56c, turn: 0xe567, invertedturn: 0xe568 };
const HEAD_W = 11.8;

const flagsOf = (base: number) => (base <= 240 ? Math.min(4, Math.round(Math.log2(480 / base))) : 0);

interface BeamGeo {
  tips: Map<number, number>; // event index -> stem tip y
  lines: { x1: number; y1: number; x2: number; y2: number }[];
}

/** Compute beam lines and stem tips for the groups of one staff in one measure. */
function computeBeams(lane: LLane, top: number, bottomIdx: number, percussion: boolean): BeamGeo {
  const evs = lane.events;
  const bottomY = top + STAFF_H;
  const geo: BeamGeo = { tips: new Map(), lines: [] };
  const yOf = (idx: number) => bottomY - (idx - bottomIdx) * (SP / 2);
  const stemX = (le: LEvent, up: boolean) => (up ? le.x + HEAD_W - 0.6 : le.x + 0.6);

  lane.beams.forEach((b: LBeam) => {
    const idxs: number[] = [];
    for (let k = b.from; k <= b.to; k++) idxs.push(k);
    const up = b.stemUp;
    const dir = up ? -1 : 1;
    const anchor = (k: number) => {
      const ev = evs[k].ev;
      if (ev.kind !== "note") return 0;
      const ids = ev.pitches.map((p) => staffIdx(percussion, p));
      return up ? yOf(Math.max(...ids)) : yOf(Math.min(...ids));
    };
    const xs = idxs.map((k) => stemX(evs[k], up));
    const x1 = xs[0];
    const x2 = xs[xs.length - 1];
    let y1 = anchor(idxs[0]) + dir * 3.5 * SP;
    let y2 = anchor(idxs[idxs.length - 1]) + dir * 3.5 * SP;
    y2 = y1 + Math.max(-1.5 * SP, Math.min(1.5 * SP, y2 - y1));
    const lineY = (x: number) => (x2 === x1 ? y1 : y1 + ((y2 - y1) * (x - x1)) / (x2 - x1));
    // Every stem keeps a minimum length of 2.5 staff spaces.
    let shift = 0;
    idxs.forEach((k, i) => {
      const need = anchor(k) + dir * 2.5 * SP;
      shift = Math.max(shift, up ? lineY(xs[i]) - need : need - lineY(xs[i]));
    });
    y1 += dir * shift;
    y2 += dir * shift;
    idxs.forEach((k, i) => geo.tips.set(k, lineY(xs[i])));

    // Beam levels: 0 = primary, 1.. = secondary (16th, 32nd, 64th).
    const flags = idxs.map((k) => flagsOf(baseOf(notatedTicks(evs[k].ev)).base));
    const maxLevel = Math.max(...flags);
    const thick = up ? BEAM_T / 2 : -BEAM_T / 2;
    for (let lv = 0; lv < maxLevel; lv++) {
      const off = -dir * lv * 1.5 * BEAM_T; // secondary beams sit toward the noteheads
      let i = 0;
      while (i < idxs.length) {
        if (flags[i] <= lv) {
          i++;
          continue;
        }
        let j = i;
        while (j + 1 < idxs.length && flags[j + 1] > lv) j++;
        let xa = xs[i];
        let xb = xs[j];
        if (i === j) {
          // Stub beam pointing at the neighbour (right unless last in the group).
          if (i < idxs.length - 1) xb = xa + 7;
          else xa = xb - 7;
        }
        geo.lines.push({ x1: xa - 0.6, y1: lineY(xa) + off + thick, x2: xb + 0.6, y2: lineY(xb) + off + thick });
        i = j + 1;
      }
    }
  });
  return geo;
}

interface Props {
  score: Score;
  layout: LayoutResult;
  cursor: Cursor;
  playing: string | null;
  inputMode: boolean;
  /** `extend` is true when Shift was held (extend the selection). */
  onPick: (part: number, voice: number, m: number, start: number, pos: number | null, extend: boolean) => void;
  selection?: Selection | null;
  /** Click on a chord symbol (screen position for the editor). */
  onChordClick?: (c: ChordMark, x: number, y: number) => void;
  /** Click on a fret number of a tablature staff: the app moves that note to another string. */
  onTabClick?: (part: number, voice: number, measure: number, start: number, midi: number) => void;
  /** String line to highlight while entering notes by fret. */
  fretLine?: { part: number; string: number } | null;
  /** Static rendering for export: fixed size, no hit areas. */
  printMode?: boolean;
  /** Draw the title and composer (first page only when printing). */
  showTitle?: boolean;
  footer?: string;
}

export function ScoreView(props: Props) {
  const clef = () => props.score.parts[0].clef;
  const timeDigits = (n: number) => [...String(n)].map((d) => g(0xe080 + Number(d))).join("");

  const renderHit = (lane: LLane, le: LEvent, top: number, m: LMeasure) => (
    <rect
      x={le.slotX - 2}
      y={top - 34}
      width={Math.max(le.slotW, 8)}
      height={STAFF_H + 68}
      fill="transparent"
      style={{ cursor: "pointer" }}
      onMouseDown={(e) => {
        const svg = (e.currentTarget as SVGRectElement).ownerSVGElement!;
        const pt = svg.createSVGPoint();
        pt.x = e.clientX;
        pt.y = e.clientY;
        const p = pt.matrixTransform(svg.getScreenCTM()!.inverse());
        const tabOnly = !!props.score.parts[lane.part].tab_only;
        props.onPick(lane.part, lane.voice, m.index, le.ev.start, tabOnly ? null : Math.round((top + STAFF_H - p.y) / (SP / 2)), e.shiftKey);
      }}
    />
  );

  const renderEvent = (lane: LLane, le: LEvent, k: number, top: number, m: LMeasure, sys: LSystem, geo: BeamGeo) => {
    const partIdx = lane.part;
    const ev = le.ev;
    const part = props.score.parts[partIdx];
    const bottomY = top + STAFF_H;
    const bottomIdx = bottomLineIndex(part.clef);
    const { base, dotted } = baseOf(notatedTicks(ev));
    const inSelection = () => {
      const s = props.selection;
      if (!s || s.part !== partIdx || s.voice !== lane.voice) return false;
      const key = m.index * 100000 + ev.start;
      return key >= s.from.m * 100000 + s.from.start && key <= s.to.m * 100000 + s.to.start;
    };
    const selected = () => inSelection() || (props.cursor.part === partIdx && props.cursor.voice === lane.voice && props.cursor.m === m.index && props.cursor.start === ev.start);
    const isPlaying = () => props.playing === `${m.index}:${ev.start}`;
    const color = () => (isPlaying() ? "var(--play)" : selected() ? "var(--accent)" : "var(--ink)");
    // The highlight also carries the staff text above the staff, so notes and rests share it.
    const hl = (
      <>
        <Show when={selected() || isPlaying()}>
          <rect x={le.slotX - 2} y={top - 12} width={Math.max(le.slotW, 8)} height={STAFF_H + 24} rx={4}
            fill={isPlaying() ? "var(--play)" : "var(--accent)"} opacity={0.14} pointer-events="none" />
        </Show>
        <Show when={ev.text}>
          <text x={le.x - 2} y={top - 14} font-family="serif" font-style="italic" font-size="13" fill="var(--ink)">{ev.text}</text>
        </Show>
      </>
    );

    if (ev.kind === "rest") {
      const fullMeasure = ev.start === 0 && ev.ticks === measureLen(props.score, m.index);
      const glyphBase = fullMeasure ? 1920 : base;
      const cx = fullMeasure ? m.x + m.headerW + (m.width - m.headerW) / 2 - 6 : le.x;
      // With several voices, odd voices move their rests down and even ones up.
      const voiceShift = lane.multi ? (lane.voice % 2 === 0 ? -2 * SP : 2 * SP) : 0;
      const y = (glyphBase >= 1920 ? top + SP : top + 2 * SP) + voiceShift;
      return (
        <g>
          {hl}
          <text x={cx} y={y} font-family="Bravura" font-size={FONT} fill={color()}>{g(REST_GLYPH[glyphBase] ?? REST_GLYPH[480])}</text>
          <Show when={dotted && !fullMeasure}>
            <text x={cx + 14} y={top + 1.5 * SP + voiceShift} font-family="Bravura" font-size={FONT} fill={color()}>{g(0xe1e7)}</text>
          </Show>
        </g>
      );
    }

    // ---- note / chord -------------------------------------------------
    const up = le.stemUp;
    const headW = base >= 1920 ? 16.8 : HEAD_W;
    const head = base >= 1920 ? 0xe0a2 : base >= 960 ? 0xe0a3 : 0xe0a4;
    const headOf = (p: Pitch) => (part.percussion && base < 1920 && DRUM_X.has(pitchMidi(p)) ? 0xe0a9 : head);
    const infos = ev.pitches.map((p, i) => {
      const pos = staffIdx(part.percussion, p) - bottomIdx;
      return { i, p, pos, y: bottomY - pos * (SP / 2), disp: false };
    });
    // Displace the upper (stem up) or lower (stem down) head of adjacent seconds.
    const ordered = [...infos].sort((a, b) => (up ? a.pos - b.pos : b.pos - a.pos));
    ordered.forEach((n, idx) => {
      const prev = ordered[idx - 1];
      if (prev && Math.abs(n.pos - prev.pos) === 1 && !prev.disp) n.disp = true;
    });
    const anyDisp = infos.some((n) => n.disp);
    const headX = (n: (typeof infos)[number]) => (n.disp ? le.x + (up ? headW : -headW) : le.x);
    const minPos = Math.min(...infos.map((n) => n.pos));
    const maxPos = Math.max(...infos.map((n) => n.pos));
    const ledgers: number[] = [];
    for (let p = -2; p >= minPos; p -= 2) ledgers.push(bottomY - p * (SP / 2));
    for (let p = 10; p <= maxPos; p += 2) ledgers.push(bottomY - p * (SP / 2));

    const flags = flagsOf(base);
    const beamed = geo.tips.has(k);
    const hasStem = base < 1920;
    const topY = Math.min(...infos.map((n) => n.y));
    const botY = Math.max(...infos.map((n) => n.y));
    const sx = up ? le.x + headW - 0.6 : le.x + 0.6;
    const defaultLen = 3.5 * SP + Math.max(0, flags - 2) * 0.45 * SP;
    const tailY = up ? botY : topY;
    const tipY = beamed ? geo.tips.get(k)! : up ? topY - defaultLen : botY + defaultLen;
    const dotX = le.x + headW + 3 + (anyDisp && up ? headW : 0);
    const dotYs = [...new Set(infos.map((n) => (n.pos % 2 === 0 ? n.y - SP / 2 : n.y)))];

    // Ties to the next event (same measure, or first event of the next measure on this system).
    const sameLane = (l: LLane) => l.part === partIdx && l.voice === lane.voice;
    const sysIdx = props.layout.systems.indexOf(sys);
    const nextInSystem: LEvent | null = lane.events[k + 1] ?? sys.measures.find((x) => x.index === m.index + 1)?.lanes.find(sameLane)?.events[0] ?? null;
    // Tie that continues on the next system: drawn to the end of this one.
    const nextSys = props.layout.systems[sysIdx + 1];
    const nextAcross: LEvent | null =
      !nextInSystem && nextSys && nextSys.measures[0].index === m.index + 1 ? (nextSys.measures[0].lanes.find(sameLane)?.events[0] ?? null) : null;
    const next = nextInSystem ?? nextAcross;
    const ties = ev.tie
      ? infos
          .filter((n) => next !== null && next.ev.kind === "note" && next.ev.pitches.some((q) => pitchMidi(q) === pitchMidi(n.p)))
          .map((n) => {
            const below = infos.length === 1 ? up : n.i < infos.length / 2;
            const x1 = headX(n) + headW + 1;
            const x2 = nextInSystem ? Math.max(x1 + 10, nextInSystem.x - 2) : m.x + m.width - 4;
            const y = n.y + (below ? 3 : -3);
            const c = below ? 8 : -8;
            return `M ${x1} ${y} Q ${(x1 + x2) / 2} ${y + c} ${x2} ${y}`;
          })
      : [];

    // Tie arriving from the previous system: drawn from the start of this one.
    const prevSys = props.layout.systems[sysIdx - 1];
    const prevEv =
      k === 0 && m === sys.measures[0] && prevSys
        ? (() => {
            const lm = prevSys.measures[prevSys.measures.length - 1];
            return lm.index === m.index - 1 ? lm.lanes.find(sameLane)?.events.slice(-1)[0] : undefined;
          })()
        : undefined;
    const leadTies =
      prevEv && prevEv.ev.kind === "note" && prevEv.ev.tie
        ? infos
            .filter((n) => prevEv.ev.kind === "note" && prevEv.ev.pitches.some((q: Pitch) => pitchMidi(q) === pitchMidi(n.p)))
            .map((n) => {
              const below = infos.length === 1 ? up : n.i < infos.length / 2;
              const x1 = m.x + m.headerW + 2;
              const x2 = Math.max(x1 + 8, headX(n) - 3);
              const y = n.y + (below ? 3 : -3);
              return `M ${x1} ${y} Q ${(x1 + x2) / 2} ${y + (below ? 8 : -8)} ${x2} ${y}`;
            })
        : [];

    return (
      <g>
        {hl}
        <For each={ledgers}>
          {(ly) => (
            <line x1={le.x - 4 - (anyDisp && !up ? headW : 0)} x2={le.x + headW + 4 + (anyDisp && up ? headW : 0)} y1={ly} y2={ly} stroke="var(--ink)" stroke-width={1.3} />
          )}
        </For>
        <For each={le.accs}>
          {(a) => (
            <text x={le.x - 13 - a.col * 10 - (anyDisp && !up ? headW : 0)} y={infos[a.pitchIndex].y} font-family="Bravura" font-size={FONT} fill={color()}>
              {g(ACC_GLYPH[a.alter])}
            </text>
          )}
        </For>
        <Show when={hasStem}>
          <line x1={sx} x2={sx} y1={tailY} y2={tipY} stroke={color()} stroke-width={1.2} />
        </Show>
        <For each={infos}>
          {(n) => <text x={headX(n)} y={n.y} font-family="Bravura" font-size={FONT} fill={color()}>{g(headOf(n.p))}</text>}
        </For>
        <Show when={flags > 0 && !beamed}>
          <text x={sx - 0.6} y={tipY} font-family="Bravura" font-size={FONT} fill={color()}>
            {g((up ? FLAG_UP : FLAG_DOWN)[flags])}
          </text>
        </Show>
        <Show when={dotted}>
          <For each={dotYs}>
            {(dy) => <text x={dotX} y={dy} font-family="Bravura" font-size={FONT} fill={color()}>{g(0xe1e7)}</text>}
          </For>
        </Show>
        <For each={leadTies}>{(d) => <path d={d} fill="none" stroke={color()} stroke-width={1.5} stroke-linecap="round" />}</For>
        <For each={ties}>{(d) => <path d={d} fill="none" stroke={color()} stroke-width={1.5} stroke-linecap="round" />}</For>
        <For each={(ev.articulations ?? []).filter((a) => ORNAMENT_GLYPH[a])}>
          {(a) => (
            <text x={le.x + headW / 2} y={Math.min(topY, bottomY - STAFF_H) - 8} text-anchor="middle" font-family="Bravura" font-size={FONT} fill={color()}>
              {g(ORNAMENT_GLYPH[a])}
            </text>
          )}
        </For>
        <For each={(ev.articulations ?? []).filter((a) => !ORNAMENT_GLYPH[a])}>
          {(a, ai) => {
            // Articulations sit on the side opposite to the stem, stacked outwards.
            const above = !up;
            const y = above ? topY - 9 - ai() * 7 : botY + 9 + ai() * 7;
            const glyph = ARTIC_GLYPH[a];
            return (
              <text x={le.x + headW / 2} y={y + (above ? 3 : 4)} text-anchor="middle" font-family="Bravura" font-size={FONT} fill={color()}>
                {g(glyph ? glyph[above ? 0 : 1] : 0x20)}
              </text>
            );
          }}
        </For>
        <For each={lyricCovers(props.score, partIdx, lane.voice).covered.get(`${m.index}:${ev.start}`) ?? []}>
          {([verse, last]) => (
            <line x1={le.slotX} x2={last ? le.x + headW + 2 : le.slotX + le.slotW} y1={bottomY + 47.5 + verse * LYRIC_LINE} y2={bottomY + 47.5 + verse * LYRIC_LINE} stroke="var(--ink)" stroke-width={0.8} />
          )}
        </For>
        <For each={ev.lyrics ?? []}>
          {(y) => (
            <>
              <Show when={lyricCovers(props.score, partIdx, lane.voice).origins.has(`${m.index}:${ev.start}:${y.verse}`)}>
                <line x1={le.x + headW / 2 + (y.text.length * LYRIC_CHAR_W) / 2 + 2} x2={le.slotX + le.slotW} y1={bottomY + 47.5 + y.verse * LYRIC_LINE} y2={bottomY + 47.5 + y.verse * LYRIC_LINE} stroke="var(--ink)" stroke-width={0.8} />
              </Show>
              <text x={le.x + headW / 2} y={bottomY + 46 + y.verse * LYRIC_LINE} text-anchor="middle" font-family="serif" font-size="12.5" fill="var(--ink)">{y.text}</text>
              <Show when={y.syllabic === "begin" || y.syllabic === "middle"}>
                <text x={(le.x + headW / 2 + (y.text.length * LYRIC_CHAR_W) / 2 + le.slotX + le.slotW + 4) / 2} y={bottomY + 46 + y.verse * LYRIC_LINE} text-anchor="middle" font-family="serif" font-size="12.5" fill="var(--ink)">-</text>
              </Show>
            </>
          )}
        </For>
        <Show when={ev.dynamic}>
          <text x={le.x - 2} y={bottomY + 31} font-family="Bravura" font-size={FONT} fill="var(--ink)">{dynamicText(ev.dynamic!)}</text>
        </Show>
      </g>
    );
  };

  /** Number (and bracket when the group is not beamed) for every tuplet group of a lane. */
  const tupletMarks = (lane: LLane, top: number, geo: BeamGeo) => {
    const part = props.score.parts[lane.part];
    const bottomY = top + STAFF_H;
    const bottomIdx = bottomLineIndex(part.clef);
    const groups = new Map<string, number[]>();
    lane.events.forEach((le, k) => {
      const t = le.ev.tuplet;
      if (!t) return;
      const key = `${t.actual}:${t.normal}:${t.start}`;
      const list = groups.get(key) ?? [];
      list.push(k);
      groups.set(key, list);
    });
    return [...groups.values()].map((idxs) => {
      const evs = idxs.map((k) => lane.events[k]);
      const t = evs[0].ev.tuplet!;
      const firstNote = evs.find((le) => le.ev.kind === "note");
      const up = lane.multi ? lane.voice % 2 === 0 : (firstNote?.stemUp ?? true);
      const tops: number[] = [];
      const bots: number[] = [];
      idxs.forEach((k) => {
        const le = lane.events[k];
        if (le.ev.kind !== "note") {
          tops.push(top + SP);
          bots.push(top + 3 * SP);
          return;
        }
        const ys = le.ev.pitches.map((p) => bottomY - (staffIdx(part.percussion, p) - bottomIdx) * (SP / 2));
        const topY = Math.min(...ys);
        const botY = Math.max(...ys);
        const stem = baseOf(notatedTicks(le.ev)).base < 1920;
        const tip = geo.tips.get(k) ?? (le.stemUp ? topY - 3.5 * SP : botY + 3.5 * SP);
        tops.push(stem && le.stemUp ? Math.min(topY, tip) : topY);
        bots.push(stem && !le.stemUp ? Math.max(botY, tip) : botY);
      });
      let yb = up ? Math.min(...tops) - 8 : Math.max(...bots) + 8;
      yb = up ? Math.min(yb, top - 6) : Math.max(yb, bottomY + 6);
      const x1 = evs[0].x - 2;
      const x2 = evs[evs.length - 1].x + HEAD_W + 2;
      const xc = (x1 + x2) / 2;
      const beamed = lane.beams.some((b) => b.from <= idxs[0] && b.to >= idxs[idxs.length - 1]);
      const cy = up || beamed ? yb : yb + 4;
      const hook = up ? 5 : -5;
      return (
        <g>
          <Show when={!beamed}>
            <path d={`M ${x1} ${cy + hook} L ${x1} ${cy} L ${xc - 9} ${cy} M ${xc + 9} ${cy} L ${x2} ${cy} L ${x2} ${cy + hook}`} fill="none" stroke="var(--ink)" stroke-width={1} />
          </Show>
          <text x={xc} y={cy + 5} text-anchor="middle" font-family="Bravura" font-size={FONT} fill="var(--ink)">{g(0xe880 + t.actual)}</text>
        </g>
      );
    });
  };

  // ---- slurs and hairpins ---------------------------------------------------
  interface EvGeom {
    x: number;
    yTop: number;
    yBot: number;
    up: boolean;
    sys: number;
  }
  const geometry = createMemo(() => {
    const events = new Map<string, EvGeom>();
    const measures = new Map<number, { sys: number; x: number; w: number; h: number; first: boolean }>();
    props.layout.systems.forEach((sys, si) => {
      const tops = props.layout.staffTops(sys);
      sys.measures.forEach((m) => {
        measures.set(m.index, { sys: si, x: m.x, w: m.width, h: m.headerW, first: m.firstInSystem });
        for (const lane of m.lanes) {
          const part = props.score.parts[lane.part];
          const top = tops[lane.part];
          const bottomY = top + STAFF_H;
          const bottomIdx = bottomLineIndex(part.clef);
          for (const le of lane.events) {
            let yTop = top + 2 * SP;
            let yBot = yTop;
            if (le.ev.kind === "note") {
              const ys = le.ev.pitches.map((p) => bottomY - (staffIdx(part.percussion, p) - bottomIdx) * (SP / 2));
              yTop = Math.min(...ys);
              yBot = Math.max(...ys);
            }
            events.set(`${lane.part}:${lane.voice}:${m.index}:${le.ev.start}`, { x: le.x, yTop, yBot, up: le.stemUp, sys: si });
          }
        }
      });
    });
    return { events, measures };
  });

  const evAt = (part: number, voice: number, pos: SpPos): EvGeom | undefined => {
    const m = props.score.parts[part]?.measures[pos.measure];
    const e = (m ? voiceEvents(m, voice) : null)?.find((x) => x.start <= pos.tick && pos.tick < x.start + x.ticks);
    return e ? geometry().events.get(`${part}:${voice}:${pos.measure}:${e.start}`) : undefined;
  };
  const sysEdges = (si: number) => {
    const ms = props.layout.systems[si].measures;
    const first = ms[0];
    const last = ms[ms.length - 1];
    return { left: first.x + first.headerW + 8, right: last.x + last.width - 6 };
  };

  /** Wavy line of a trill. */
  const wavy = (x1: number, x2: number, y: number) => {
    let d = `M ${x1} ${y}`;
    for (let x = x1 + 2; x <= x2; x += 2) d += ` L ${x} ${y + 2.5 * Math.sin(((x - x1) / 8) * 2 * Math.PI)}`;
    return d;
  };

  const arc = (x1: number, y1: number, x2: number, y2: number, below: boolean) => {
    const dx = Math.max(x2 - x1, 8);
    const h = (below ? 1 : -1) * Math.max(6, Math.min(dx * 0.1, 16)) * 1.3;
    return `M ${x1} ${y1} C ${x1 + dx * 0.2} ${y1 + h}, ${x2 - dx * 0.2} ${y2 + h}, ${x2} ${y2}`;
  };

  /** First/second ending bracket above the first staff, split over systems when needed. */
  const renderVolta = (sp: Spanner) => {
    const g = geometry();
    const last = sp.end.tick === 0 && sp.end.measure > sp.start.measure ? sp.end.measure - 1 : sp.end.measure;
    const nums = [1, 2, 3, 4, 5, 6, 7, 8].filter((n) => ((sp.endings ?? 0) & (1 << (n - 1))) !== 0);
    const label = nums.length ? nums.join(", ") + "." : "";
    const closed = (props.score.parts[0].measures[last]?.end_repeat ?? 0) > 0;
    const segs: { sys: number; x1: number; x2: number; first: boolean; end: boolean }[] = [];
    for (let m = sp.start.measure; m <= last; m++) {
      const b = g.measures.get(m);
      if (!b) continue;
      const seg = segs.find((s) => s.sys === b.sys);
      const left = m === sp.start.measure ? b.x : b.first ? b.x + b.h : b.x;
      if (seg) seg.x2 = b.x + b.w;
      else segs.push({ sys: b.sys, x1: left, x2: b.x + b.w, first: m === sp.start.measure, end: false });
    }
    if (segs.length) segs[segs.length - 1].end = true;
    return (
      <For each={segs}>
        {(s) => {
          const y = props.layout.staffTops(props.layout.systems[s.sys])[0] - 40;
          const d = `${s.first ? `M ${s.x1 + 2} ${y + 12} L ${s.x1 + 2} ${y}` : `M ${s.x1 + 2} ${y}`} L ${s.x2 - 2} ${y}${s.end && closed ? ` L ${s.x2 - 2} ${y + 12}` : ""}`;
          return (
            <g>
              <path d={d} fill="none" stroke="var(--ink)" stroke-width={1} />
              <Show when={s.first}>
                <text x={s.x1 + 7} y={y + 12} font-size="11" font-family="system-ui" fill="var(--ink)">{label}</text>
              </Show>
            </g>
          );
        }}
      </For>
    );
  };

  /** Octave lines (8va...) and pedal: a label, a dashed or solid line and a hook at the end. */
  const renderLine = (pi: number, sp: Spanner, a: EvGeom) => {
    const g = geometry();
    const endLen = measureLen(props.score, sp.end.measure);
    const endBox = g.measures.get(sp.end.measure);
    let endX: number;
    let endSys: number;
    if (sp.end.tick >= endLen) {
      if (!endBox) return null;
      endX = endBox.x + endBox.w - 5;
      endSys = endBox.sys;
    } else {
      const b = evAt(pi, sp.voice, sp.end);
      if (!b) return null;
      endX = b.x - 3;
      endSys = b.sys;
    }
    const pedal = sp.kind === "pedal";
    const trill = sp.kind === "trill";
    const above = sp.kind === "ottava8va" || sp.kind === "ottava15ma" || trill;
    const topOf = (si: number) => props.layout.staffTops(props.layout.systems[si])[pi];
    const yLine = (si: number) => (pedal ? topOf(si) + STAFF_H + 52 : trill ? topOf(si) - 14 : above ? topOf(si) - 20 : topOf(si) + STAFF_H + 40);
    const label = trill ? 0xe566 : pedal ? 0xe650 : ({ ottava8va: 0xe511, ottava8vb: 0xe512, ottava15ma: 0xe515, ottava15mb: 0xe516 } as Record<string, number>)[sp.kind];
    const hook = above ? 6 : -6; // towards the staff
    const labelW = trill ? 22 : pedal ? 28 : sp.kind.includes("15") ? 30 : 24;
    const segs =
      a.sys === endSys
        ? [{ x1: a.x - 2, x2: endX, y: yLine(a.sys), label: true, hook: true }]
        : [
            { x1: a.x - 2, x2: sysEdges(a.sys).right, y: yLine(a.sys), label: true, hook: false },
            { x1: sysEdges(endSys).left, x2: endX, y: yLine(endSys), label: false, hook: true },
          ];
    return (
      <For each={segs}>
        {(s) => (
          <g>
            <Show when={s.label}>
              <text x={s.x1} y={s.y + (above ? 5 : pedal ? 4 : 6)} font-family="Bravura" font-size={FONT} fill="var(--ink)">{g2(label)}</text>
            </Show>
            <path
              d={trill ? wavy(s.x1 + (s.label ? labelW : 0), s.x2, s.y) : `M ${s.x1 + (s.label ? labelW : 0)} ${s.y} L ${s.x2} ${s.y}${s.hook ? ` L ${s.x2} ${s.y + hook}` : ""}`}
              fill="none"
              stroke="var(--ink)"
              stroke-width={trill ? 1.4 : 1}
              stroke-dasharray={pedal || trill ? undefined : "4 3"}
            />
          </g>
        )}
      </For>
    );
  };

  const renderSpanner = (pi: number, sp: Spanner) => {
    const a = evAt(pi, sp.voice, sp.start);
    if (!a) return null;
    if (sp.kind === "slur") {
      const b = evAt(pi, sp.voice, sp.end);
      if (!b) return null;
      const below = a.up;
      const ya = below ? a.yBot + 5 : a.yTop - 5;
      const yb = below ? b.yBot + 5 : b.yTop - 5;
      const x1 = a.x + HEAD_W / 2;
      const x2 = b.x + HEAD_W / 2;
      const paths =
        a.sys === b.sys
          ? [arc(x1, ya, x2, yb, below)]
          : [arc(x1, ya, sysEdges(a.sys).right, ya, below), arc(sysEdges(b.sys).left, yb, x2, yb, below)];
      // the tablature staff shows the slur above its own lines (alone when the standard staff is hidden)
      const part = props.score.parts[pi];
      const tabTop = (si: number) => {
        const top = props.layout.staffTops(props.layout.systems[si])[pi];
        return part.tab_only ? top + (STAFF_H - tabHeight(part.tuning!.length)) / 2 : top + props.layout.tabDy;
      };
      const tabPaths =
        part.tuning && !part.percussion
          ? a.sys === b.sys
            ? [arc(a.x + 5, tabTop(a.sys) - 6, b.x + 5, tabTop(b.sys) - 6, false)]
            : [arc(a.x + 5, tabTop(a.sys) - 6, sysEdges(a.sys).right, tabTop(a.sys) - 6, false), arc(sysEdges(b.sys).left, tabTop(b.sys) - 6, b.x + 5, tabTop(b.sys) - 6, false)]
          : [];
      return (
        <>
          <For each={part.tab_only ? [] : paths}>{(d) => <path d={d} fill="none" stroke="var(--ink)" stroke-width={1.6} stroke-linecap="round" />}</For>
          <For each={tabPaths}>{(d) => <path d={d} fill="none" stroke="var(--ink)" stroke-width={1.3} stroke-linecap="round" />}</For>
        </>
      );
    }
    if (sp.kind === "volta") return pi === 0 ? renderVolta(sp) : null;
    if (sp.kind !== "crescendo" && sp.kind !== "decrescendo") return renderLine(pi, sp, a);
    // hairpin
    const g = geometry();
    const endLen = measureLen(props.score, sp.end.measure);
    const endBox = g.measures.get(sp.end.measure);
    let endX: number;
    let endSys: number;
    if (sp.end.tick >= endLen) {
      if (!endBox) return null;
      endX = endBox.x + endBox.w - 5;
      endSys = endBox.sys;
    } else {
      const b = evAt(pi, sp.voice, sp.end);
      if (!b) return null;
      endX = b.x - 3;
      endSys = b.sys;
    }
    const ySys = (si: number) => props.layout.staffTops(props.layout.systems[si])[pi] + STAFF_H + 26 + (props.score.parts[pi].tab_only ? TAB_LYRIC_DY + 6 : 0);
    const half = 6;
    const wedge = (x1: number, x2: number, y: number, h1: number, h2: number) => [
      `M ${x1} ${y - h1} L ${x2} ${y - h2}`,
      `M ${x1} ${y + h1} L ${x2} ${y + h2}`,
    ];
    const grow = sp.kind === "crescendo";
    const [h0, h1] = grow ? [0, half] : [half, 0];
    const x1 = a.x;
    const segs =
      a.sys === endSys
        ? wedge(x1, endX, ySys(a.sys), h0, h1)
        : [
            ...wedge(x1, sysEdges(a.sys).right, ySys(a.sys), h0, (h0 + h1) / 2),
            ...wedge(sysEdges(endSys).left, endX, ySys(endSys), (h0 + h1) / 2, h1),
          ];
    return <For each={segs}>{(d) => <path d={d} fill="none" stroke="var(--ink)" stroke-width={1.1} />}</For>;
  };

  /** Rhythm of a tablature-only part: stems and beams under the staff, rests, and the cursor / playback highlight. */
  const tabRhythm = (pi: number, top: number, ty: number, n: number, sys: LSystem) => {
    const bottom = ty + tabHeight(n);
    const lyricY = top + STAFF_H + TAB_LYRIC_DY;
    const stemEnd = bottom + 22;
    const flagsOf = (e: EventData) => {
      if (e.kind !== "note") return 0;
      const { base } = baseOf(notatedTicks(e));
      return base >= 480 ? 0 : base === 240 ? 1 : base === 120 ? 2 : base === 60 ? 3 : 4;
    };
    return (
      <For each={sys.measures}>
        {(m) => (
          <For each={m.lanes.filter((l) => l.part === pi)}>
            {(lane) => {
              const evs = lane.events;
              const sx = (le: LEvent) => le.x + 5;
              // beams: runs of neighbouring notes of the same beat with at least `l` flags; a lone one gets a flag
              const lines: { x1: number; y1: number; x2: number; y2: number }[] = [];
              for (let l = 1; l <= 4; l++) {
                let i = 0;
                while (i < evs.length) {
                  if (flagsOf(evs[i].ev) < l) {
                    i++;
                    continue;
                  }
                  let j = i;
                  while (j + 1 < evs.length && flagsOf(evs[j + 1].ev) >= l && Math.floor(evs[j + 1].ev.start / PPQ) === Math.floor(evs[i].ev.start / PPQ)) j++;
                  const y = stemEnd - (l - 1) * 3.5;
                  if (j > i) lines.push({ x1: sx(evs[i]), y1: y, x2: sx(evs[j]), y2: y });
                  else lines.push({ x1: sx(evs[i]), y1: y, x2: sx(evs[i]) + 5, y2: y - 6 });
                  i = j + 1;
                }
              }
              // tuplets: the number goes under the stems of each group
              const tupletGroups: { x1: number; x2: number; n: number; key: string }[] = [];
              for (const le of evs) {
                const t = le.ev.tuplet;
                if (!t) continue;
                const key = `${t.start}:${t.actual}`;
                const last = tupletGroups[tupletGroups.length - 1];
                if (last && last.key === key) last.x2 = sx(le);
                else tupletGroups.push({ x1: sx(le), x2: sx(le), n: t.actual, key });
              }
              return (
                <g>
                  <For each={evs}>
                    {(le) => {
                      const ev = le.ev;
                      const { base, dotted } = baseOf(notatedTicks(ev));
                      const sel = () => {
                        const s = props.selection;
                        const key = m.index * 100000 + ev.start;
                        if (s && s.part === pi && s.voice === lane.voice) return key >= s.from.m * 100000 + s.from.start && key <= s.to.m * 100000 + s.to.start;
                        return props.cursor.part === pi && props.cursor.voice === lane.voice && props.cursor.m === m.index && props.cursor.start === ev.start;
                      };
                      const playing = () => props.playing === `${m.index}:${ev.start}`;
                      const full = ev.kind === "rest" && ev.start === 0 && ev.ticks === measureLen(props.score, m.index);
                      const cx = full ? m.x + m.headerW + (m.width - m.headerW) / 2 - 6 : le.x;
                      const restBase = full ? 1920 : base;
                      return (
                        <g>
                          <Show when={sel() || playing()}>
                            <rect x={le.slotX - 2} y={ty - 12} width={Math.max(le.slotW, 8)} height={tabHeight(n) + 40} rx={4} fill={playing() ? "var(--play)" : "var(--accent)"} opacity={0.14} pointer-events="none" />
                          </Show>
                          <Show when={ev.kind === "note" && base < 1920}>
                            <line x1={sx(le)} x2={sx(le)} y1={bottom + 3} y2={stemEnd} stroke="var(--ink)" stroke-width={1} />
                          </Show>
                          <Show when={ev.kind === "note" && dotted}>
                            <circle cx={sx(le) + 4} cy={stemEnd - 3} r={1.4} fill="var(--ink)" />
                          </Show>
                          <Show when={ev.kind === "rest"}>
                            <text x={cx} y={ty + tabHeight(n) / 2 + (restBase >= 1920 ? -SP : 0)} font-family="Bravura" font-size={FONT} fill="var(--ink)">{g(REST_GLYPH[restBase] ?? REST_GLYPH[480])}</text>
                          </Show>
                          <Show when={ev.text}>
                            <text x={le.x - 2} y={ty - 18} font-family="serif" font-style="italic" font-size="13" fill="var(--ink)">{ev.text}</text>
                          </Show>
                          <Show when={ev.dynamic}>
                            <text x={le.x - 2} y={top + STAFF_H + 31 + TAB_LYRIC_DY} font-family="Bravura" font-size={FONT} fill="var(--ink)">{dynamicText(ev.dynamic!)}</text>
                          </Show>
                          <For each={lyricCovers(props.score, pi, lane.voice).covered.get(`${m.index}:${ev.start}`) ?? []}>
                            {([verse, last]) => (
                              <line x1={le.slotX} x2={last ? le.x + HEAD_W + 2 : le.slotX + le.slotW} y1={lyricY + 47.5 + verse * LYRIC_LINE} y2={lyricY + 47.5 + verse * LYRIC_LINE} stroke="var(--ink)" stroke-width={0.8} />
                            )}
                          </For>
                          <For each={ev.lyrics ?? []}>
                            {(y) => (
                              <>
                                <Show when={lyricCovers(props.score, pi, lane.voice).origins.has(`${m.index}:${ev.start}:${y.verse}`)}>
                                  <line x1={le.x + HEAD_W / 2 + (y.text.length * LYRIC_CHAR_W) / 2 + 2} x2={le.slotX + le.slotW} y1={lyricY + 47.5 + y.verse * LYRIC_LINE} y2={lyricY + 47.5 + y.verse * LYRIC_LINE} stroke="var(--ink)" stroke-width={0.8} />
                                </Show>
                                <text x={le.x + HEAD_W / 2} y={lyricY + 46 + y.verse * LYRIC_LINE} text-anchor="middle" font-family="serif" font-size="12.5" fill="var(--ink)">{y.text}</text>
                                <Show when={y.syllabic === "begin" || y.syllabic === "middle"}>
                                  <text x={(le.x + HEAD_W / 2 + (y.text.length * LYRIC_CHAR_W) / 2 + le.slotX + le.slotW + 4) / 2} y={lyricY + 46 + y.verse * LYRIC_LINE} text-anchor="middle" font-family="serif" font-size="12.5" fill="var(--ink)">-</text>
                                </Show>
                              </>
                            )}
                          </For>
                        </g>
                      );
                    }}
                  </For>
                  <For each={lines}>{(l) => <line x1={l.x1} y1={l.y1} x2={l.x2} y2={l.y2} stroke="var(--ink)" stroke-width={2} />}</For>
                  <For each={tupletGroups}>
                    {(t) => <text x={(t.x1 + t.x2) / 2} y={stemEnd + 11} text-anchor="middle" font-family="serif" font-style="italic" font-size="11" fill="var(--ink)">{t.n}</text>}
                  </For>
                </g>
              );
            }}
          </For>
        )}
      </For>
    );
  };

  /** Ties between equal frets and guitar techniques (hammer-on / pull-off, slide, bend) of the note `le` of a tablature staff. */
  const tabMarks = (pi: number, lane: LLane, k: number, le: LEvent, m: LMeasure, sys: LSystem, lineY: (string: number) => number, positions: Map<string, ({ string: number; fret: number } | null)[]>) => {
    const ev = le.ev;
    if (ev.kind !== "note") return null;
    const mine = positions.get(`${m.index}:${ev.start}`) ?? [];
    const placed = mine.flatMap((f, i) => (f ? [{ f, i }] : []));
    if (!placed.length) return null;
    const sameMeasure = lane.events[k + 1];
    const nle = sameMeasure ?? sys.measures.find((x) => x.index === m.index + 1)?.lanes.find((l) => l.part === pi && l.voice === lane.voice)?.events[0] ?? null;
    const nIdx = sameMeasure ? m.index : m.index + 1;
    const nEv = nle && nle.ev.kind === "note" ? nle.ev : null;
    const nFrets = nEv ? (positions.get(`${nIdx}:${nEv.start}`) ?? []) : [];
    const x1 = le.x + 5;
    const x2 = nle ? nle.x + 5 : x1 + 22;
    const top = placed[placed.length - 1]; // the highest pitch carries the technique
    const yMine = lineY(top.f.string);
    const nTop = nFrets.flatMap((f) => (f ? [f] : [])).slice(-1)[0];
    const yNext = nTop ? lineY(nTop.string) : yMine;
    const mids = (e: EventData) => (e.kind === "note" ? (e.snd ?? e.pitches.map(pitchMidi)) : []);
    const myMidi = Math.max(...mids(ev));
    const nextMidi = nEv ? Math.max(...mids(nEv)) : myMidi;
    const out: any[] = [];
    // ties: an arc over the line of every tied pitch that continues in the next note
    if (ev.tie && nEv) {
      const nm = mids(nEv);
      const m0 = mids(ev);
      for (const p of placed) {
        if (!nm.includes(m0[p.i])) continue;
        const y = lineY(p.f.string) - 5;
        out.push(<path d={`M ${x1 + 5} ${y} Q ${(x1 + x2) / 2 + 2} ${y - 7} ${x2 - 3} ${y}`} fill="none" stroke="var(--ink)" stroke-width={1.1} stroke-linecap="round" />);
      }
    }
    const tech = ev.tech;
    if (tech === "hammer" && nEv) {
      const yA = Math.min(yMine, yNext) - 8;
      const xm = (x1 + x2) / 2 + 2;
      out.push(<path d={`M ${x1 + 4} ${yA} Q ${xm} ${yA - 9} ${x2 + 2} ${yA}`} fill="none" stroke="var(--ink)" stroke-width={1.1} stroke-linecap="round" />);
      out.push(<text x={xm} y={yA - 6} text-anchor="middle" font-family="serif" font-style="italic" font-size="9" fill="var(--ink)">{nextMidi >= myMidi ? "H" : "P"}</text>);
    } else if (tech === "slide" && nEv) {
      const dir = nextMidi >= myMidi ? 1 : -1; // up in pitch = up on the page
      out.push(<line x1={x1 + 8} y1={yMine + 3 * dir} x2={x2 - 6} y2={yNext - 3 * dir} stroke="var(--ink)" stroke-width={1.2} />);
    } else if (tech && typeof tech === "object") {
      const n = tech.bend;
      const label = ["", "½", "1", "1½", "2"][Math.max(1, Math.min(4, n))];
      const xb = x1 + 17;
      out.push(<path d={`M ${x1 + 7} ${yMine - 2} C ${x1 + 14} ${yMine - 2}, ${xb} ${yMine - 5}, ${xb} ${yMine - 12}`} fill="none" stroke="var(--ink)" stroke-width={1.1} />);
      out.push(<path d={`M ${xb - 2.5} ${yMine - 10} L ${xb} ${yMine - 15} L ${xb + 2.5} ${yMine - 10} Z`} fill="var(--ink)" />);
      out.push(<text x={xb} y={yMine - 18} text-anchor="middle" font-family="system-ui" font-size="9" fill="var(--ink)">{label}</text>);
    }
    return out;
  };

  /** Tablature staff under a part with a tuning: one line per string, fret numbers on the lines. */
  const tabStaff = (pi: number, top: number, sys: LSystem) => {
    const part = props.score.parts[pi];
    const n = part.tuning!.length;
    // under the staff, or centred in the staff's place when the part shows only the tablature
    const ty = part.tab_only ? top + (STAFF_H - tabHeight(n)) / 2 : top + props.layout.tabDy;
    const x1 = sys.measures[0].x;
    const last = sys.measures[sys.measures.length - 1];
    const x2 = last.x + last.width;
    const lineY = (string: number) => ty + (string - 1) * TAB_SP;
    return (
      <g>
        <For each={Array.from({ length: n }, (_, i) => i)}>
          {(i) => <line x1={x1} x2={x2} y1={ty + i * TAB_SP} y2={ty + i * TAB_SP} stroke="var(--ink)" stroke-width={1} />}
        </For>
        <line x1={x1} x2={x1} y1={ty} y2={ty + tabHeight(n)} stroke="var(--ink)" stroke-width={1.4} />
        <For each={sys.measures}>
          {(m) => <line x1={m.x + m.width} x2={m.x + m.width} y1={ty} y2={ty + tabHeight(n)} stroke="var(--ink)" stroke-width={1} />}
        </For>
        <text x={x1 + 8} y={ty + tabHeight(n) / 2 - 4} font-family="serif" font-weight="bold" font-size="12" fill="var(--ink)">T</text>
        <text x={x1 + 8} y={ty + tabHeight(n) / 2 + 8} font-family="serif" font-weight="bold" font-size="12" fill="var(--ink)">A</text>
        <text x={x1 + 8} y={ty + tabHeight(n) / 2 + 20} font-family="serif" font-weight="bold" font-size="12" fill="var(--ink)">B</text>
        <Show when={part.tab_only}>{tabRhythm(pi, top, ty, n, sys)}</Show>
        <Show when={props.fretLine && props.fretLine.part === pi}>
          <line x1={x1} x2={x2} y1={lineY(props.fretLine!.string)} y2={lineY(props.fretLine!.string)} stroke="var(--accent)" stroke-width={2.2} opacity={0.55} pointer-events="none" />
        </Show>
        <For each={sys.measures}>
          {(m) => (
            <For each={m.lanes.filter((l) => l.part === pi)}>
              {(lane) => {
                const positions = tabPositions(props.score, pi, lane.voice);
                return (
                  <For each={lane.events}>
                    {(le, ki) => {
                      const frets = positions.get(`${m.index}:${le.ev.start}`);
                      const ev = le.ev;
                      return (
                        <>
                        <Show when={frets && ev.kind === "note"}>{tabMarks(pi, lane, ki(), le, m, sys, lineY, positions)}</Show>
                        <Show when={frets && ev.kind === "note"}>
                          <For each={frets!}>
                            {(f, k) => {
                              const midi = ev.kind === "note" ? (ev.snd ?? ev.pitches.map(pitchMidi))[k()] : 0;
                              const x = le.x + 5;
                              const y = f ? lineY(f.string) : ty + tabHeight(n) / 2;
                              const label = f ? String(f.fret) : "×";
                              return (
                                <g style={props.onTabClick ? { cursor: "pointer" } : undefined} onClick={() => props.onTabClick?.(pi, lane.voice, m.index, ev.start, midi)}>
                                  <rect x={x - 1 - (label.length - 1) * 3} y={y - 5.5} width={5 + label.length * 5.2} height={11} fill="#fbfaf6" />
                                  <text x={x + 1.5} y={y + 4} text-anchor="middle" font-family="system-ui" font-size="11" fill="var(--ink)">{label}</text>
                                  <title>{f ? `Cuerda ${f.string}, traste ${f.fret}${ev.strings?.length ? " (fijada)" : ""}. Clic: otra cuerda` : "Fuera de la afinación"}</title>
                                </g>
                              );
                            }}
                          </For>
                        </Show>
                        </>
                      );
                    }}
                  </For>
                );
              }}
            </For>
          )}
        </For>
      </g>
    );
  };

  /** Key signature at `x`; with `prev` (a key change) the signs it cancels are drawn first as naturals. */
  const keySig = (top: number, x: number, f: number, prev = 0) => {
    const posList = f > 0 ? SHARP_POS : FLAT_POS;
    const shift = clef() === "bass" ? -2 : 0;
    const nat = cancelledSigns(prev, f);
    const natFrom = Math.sign(prev) === Math.sign(f) ? Math.abs(f) : 0;
    const natPos = prev > 0 ? SHARP_POS : FLAT_POS;
    const y = (list: number[], i: number) => top + STAFF_H - (list[i] + shift) * (SP / 2);
    return (
      <>
        <For each={Array.from({ length: nat }, (_, i) => i)}>
          {(j) => (
            <text x={x + j * 11} y={y(natPos, natFrom + j)} font-family="Bravura" font-size={FONT} fill="var(--ink)">{g(0xe261)}</text>
          )}
        </For>
        <For each={Array.from({ length: Math.abs(f) }, (_, i) => i)}>
          {(i) => (
            <text x={x + (nat + i) * 11} y={y(posList, i)} font-family="Bravura" font-size={FONT} fill="var(--ink)">
              {g(f > 0 ? 0xe262 : 0xe260)}
            </text>
          )}
        </For>
      </>
    );
  };

  return (
    <svg class="score" viewBox={`0 0 ${props.layout.width} ${props.layout.height}`} width={props.printMode ? String(props.layout.width) : "100%"} height={props.printMode ? String(props.layout.height) : undefined} role="img" aria-label="Partitura">
      <Show when={props.showTitle !== false}>
        <text x={props.layout.width / 2} y={50} text-anchor="middle" font-size="26" font-weight="600" fill="var(--ink)" font-family="Georgia, serif">
          {props.score.title}
        </text>
        <Show when={props.score.composer}>
          <text x={props.layout.width - 40} y={78} text-anchor="end" font-size="13" fill="var(--ink-dim)" font-family="Georgia, serif">{props.score.composer}</text>
        </Show>
      </Show>
      <Show when={props.footer}>
        <text x={props.layout.width / 2} y={props.layout.height - 30} text-anchor="middle" font-size="11" fill="var(--ink-dim)" font-family="system-ui">{props.footer}</text>
      </Show>
      <For each={props.layout.systems}>
        {(sys) => {
          const tops = props.layout.staffTops(sys);
          const first = sys.measures[0];
          const last = sys.measures[sys.measures.length - 1];
          return (
            <g>
              <For each={props.score.parts}>
                {(part, pi) => {
                  const top = tops[pi()];
                  return (
                    <g>
                      <Show when={!part.tab_only}>
                        <For each={[0, 1, 2, 3, 4]}>
                          {(l) => <line x1={first.x} x2={last.x + last.width} y1={top + l * SP} y2={top + l * SP} stroke="var(--ink)" stroke-width={1} />}
                        </For>
                        <text x={first.x + 6} y={part.percussion ? top + 2 * SP : part.clef === "treble" ? top + 3 * SP : top + SP} font-family="Bravura" font-size={FONT} fill="var(--ink)">
                          {g(part.percussion ? 0xe069 : part.clef === "treble" ? 0xe050 : 0xe062)}
                        </text>
                      </Show>
                      <Show when={sys === props.layout.systems[0] || props.score.parts.length > 1}>
                        <text x={first.x + 4} y={top - 20} font-size="10" fill="var(--ink-dim)" font-family="system-ui" font-style="italic">{part.name}</text>
                      </Show>
                      <Show when={!part.percussion && !part.tab_only}>{keySig(top, first.x + 40, partKeyAt(props.score, part, first.index))}</Show>
                      <For each={sys.measures}>
                        {(m) => {
                          const lanes = m.lanes.filter((l) => l.part === pi());
                          // Hit areas: the active voice goes last so it wins when voices overlap.
                          const hitOrder = [...lanes].sort((a, b) => Number(a.voice === props.cursor.voice) - Number(b.voice === props.cursor.voice));
                          return (
                            <g>
                              <For each={part.tab_only ? [] : lanes}>
                                {(lane) => {
                                  const geo = computeBeams(lane, top, bottomLineIndex(part.clef), part.percussion);
                                  return (
                                    <g>
                                      <For each={lane.events}>{(le, k) => renderEvent(lane, le, k(), top, m, sys, geo)}</For>
                                      {tupletMarks(lane, top, geo)}
                                      <For each={geo.lines}>
                                        {(l) => <line x1={l.x1} y1={l.y1} x2={l.x2} y2={l.y2} stroke="var(--ink)" stroke-width={BEAM_T} />}
                                      </For>
                                    </g>
                                  );
                                }}
                              </For>
                              <Show when={m.showKey && !part.percussion && !part.tab_only && partKeyAt(props.score, part, m.index) !== partKeyAt(props.score, part, m.index - 1)}>
                                {keySig(top, m.x + 6, partKeyAt(props.score, part, m.index), partKeyAt(props.score, part, m.index - 1))}
                              </Show>
                              <Show when={m.showTime}>
                                <text x={m.x + m.headerW - 26} y={top + SP} font-family="Bravura" font-size={FONT} fill="var(--ink)">{timeDigits(m.time.num)}</text>
                                <text x={m.x + m.headerW - 26} y={top + 3 * SP} font-family="Bravura" font-size={FONT} fill="var(--ink)">{timeDigits(m.time.den)}</text>
                              </Show>
                              <Show when={!part.tab_only && props.cursor.part === pi() && props.cursor.m === m.index && !lanes.some((l) => l.voice === props.cursor.voice)}>
                                <rect x={m.x + m.headerW + 6} y={top - 6} width={16} height={STAFF_H + 12} rx={3} fill="var(--accent)" opacity={0.25} pointer-events="none" />
                              </Show>
                              <Show when={!props.printMode}>
                                <For each={hitOrder}>{(lane) => <For each={lane.events}>{(le) => renderHit(lane, le, top, m)}</For>}</For>
                              </Show>
                              {(() => {
                                const cur = props.score.parts[pi()].measures[m.index];
                                const next = props.score.parts[pi()].measures[m.index + 1];
                                const sameSystem = !!sys.measures.find((x) => x.index === m.index + 1);
                                const endsHere = (cur?.end_repeat ?? 0) > 0;
                                const startsNext = !!next?.start_repeat && sameSystem;
                                const bx = m.x + m.width;
                                const gy = top + STAFF_H;
                                return (
                                  <g>
                                    <Show when={endsHere && startsNext}>
                                      <text x={bx} y={gy} text-anchor="middle" font-family="Bravura" font-size={FONT} fill="var(--ink)">{g(0xe042)}</text>
                                    </Show>
                                    <Show when={endsHere && !startsNext}>
                                      <text x={bx + 1} y={gy} text-anchor="end" font-family="Bravura" font-size={FONT} fill="var(--ink)">{g(0xe041)}</text>
                                    </Show>
                                    <Show when={pi() === 0 && ((cur?.markers?.length ?? 0) > 0 || cur?.jump)}>
                                      <For each={(cur?.markers ?? []).filter((k) => k === "Segno" || k === "Coda")}>
                                        {(k) => (
                                          <text x={m.x + (m.firstInSystem ? m.headerW : 0) + 16} y={top - 12} font-family="Bravura" font-size={String(SP * 2.4)} fill="var(--ink)">{g(k === "Segno" ? 0xe047 : 0xe048)}</text>
                                        )}
                                      </For>
                                      <text x={bx - 4} y={top - 10} text-anchor="end" font-size="12" font-style="italic" font-family="serif" fill="var(--ink)">
                                        {[...(cur?.markers ?? []).filter((k) => k === "ToCoda").map(() => "To Coda"), ...(cur?.markers ?? []).filter((k) => k === "Fine").map(() => "Fine"), ...(cur?.jump ? [JUMP_TEXT[cur.jump]] : [])].join("  ")}
                                      </text>
                                    </Show>
                                    <Show when={cur?.start_repeat && !(m.index > 0 && (props.score.parts[pi()].measures[m.index - 1]?.end_repeat ?? 0) > 0 && !m.firstInSystem)}>
                                      <text x={m.x + (m.firstInSystem ? m.headerW : 0) - 1} y={gy} font-family="Bravura" font-size={FONT} fill="var(--ink)">{g(0xe040)}</text>
                                    </Show>
                                  </g>
                                );
                              })()}
                              <line x1={m.x + m.width} x2={m.x + m.width} y1={top} y2={top + STAFF_H} stroke="var(--ink)" stroke-width={1} />
                              <Show when={m === last && m.index === props.score.parts[0].measures.length - 1}>
                                <line x1={m.x + m.width + 4} x2={m.x + m.width + 4} y1={top} y2={top + STAFF_H} stroke="var(--ink)" stroke-width={3.5} />
                              </Show>
                            </g>
                          );
                        }}
                      </For>
                      <line x1={first.x} x2={first.x} y1={top} y2={top + STAFF_H} stroke="var(--ink)" stroke-width={1.4} />
                      <Show when={part.tuning && !part.percussion}>{tabStaff(pi(), top, sys)}</Show>
                    </g>
                  );
                }}
              </For>
              <Show when={props.score.parts.length > 1}>
                <line x1={first.x} x2={first.x} y1={tops[0]} y2={tops[tops.length - 1] + STAFF_H} stroke="var(--ink)" stroke-width={1.4} />
              </Show>
              <For each={(props.score.chords ?? []).filter((c) => sys.measures.some((m) => m.index === c.measure))}>
                {(c) => {
                  const m = sys.measures.find((x) => x.index === c.measure)!;
                  // x del primer evento del primer pentagrama que empieza en el tick del acorde o después
                  const evs = m.lanes.filter((l) => l.part === 0).flatMap((l) => l.events).filter((le) => le.ev.start >= c.tick).sort((a, b) => a.ev.start - b.ev.start);
                  const x = evs.length ? evs[0].x : m.x + m.headerW + 6;
                  // sube la etiqueta si hay notas (o sus plicas, ~7 medios espacios) por encima del pentagrama en este compás
                  const idxTop = Math.max(-1e9, ...m.lanes.filter((l) => l.part === 0).flatMap((l) => l.events).map((le) => (le.ev.kind === "note" ? Math.max(...le.ev.pitches.map((q) => staffIdx(props.score.parts[0].percussion, q))) + 7 : -1e9)));
                  const lift = Math.min(30, Math.max(0, idxTop - (bottomLineIndex(clef()) + 8) + 1) * (SP / 2));
                  return (
                    <text x={x - 2} y={tops[0] - 8 - lift} font-size="13" font-weight="600" fill="var(--ink)" font-family="system-ui" style={props.onChordClick ? { cursor: "pointer" } : undefined} onClick={(e) => props.onChordClick?.(c, e.clientX, e.clientY)}>{chordName(c, keyAt(props.score, c.measure), props.score.parts[0]?.transpose)}<title>{c.manual ? "Fijado a mano (se conserva al actualizar)" : "Detectado (se recalcula al actualizar)"}</title></text>
                  );
                }}
              </For>
              <For each={sys.measures}>
                {(m) => (
                  <text x={m.x + m.headerW + 4} y={tops[0] - 22} font-size="10" fill="var(--ink-dim)" font-family="system-ui">{m.index + 1}</text>
                )}
              </For>
            </g>
          );
        }}
      </For>
      <For each={props.score.parts}>{(p, pi) => <For each={p.spanners ?? []}>{(sp) => renderSpanner(pi(), sp)}</For>}</For>
    </svg>
  );
}
