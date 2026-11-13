import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { api, AudioOptions } from "./api";
import { drumFromPos, DRUM_KEYS } from "./drum";
import { INSTRUMENTS } from "./instruments";
import { MAX_FRET, OCTAVE_BELOW, playableStrings, tabPositions, TUNINGS } from "./tab";
import { keyAlter, layoutScore } from "./layout";
import { scoreToSvgPages } from "./pdfExport";
import { Player } from "./playback";
import { ScoreView } from "./ScoreView";
import { ArticKind, chordName, Cursor, displayScore, DYNAMICS, Jump, JUMP_TEXT, Marker, measureLen, keyAt, ottavaShiftAt, partKeyAt, Pitch, pitchMidi, Score, Selection, soundingPitch, SpannerKind, Tech, timeAt, voiceEvents } from "./types";
import "./styles.css";

const BASE: Record<number, number> = { 3: 120, 4: 240, 5: 480, 6: 960, 7: 1920 };
const DUR_LABEL: Record<number, string> = { 3: "𝅘𝅥𝅯", 4: "♪", 5: "♩", 6: "𝅗𝅥", 7: "𝅝" };
const KEY_NAMES = ["Do♭ M", "Sol♭ M", "Re♭ M", "La♭ M", "Mi♭ M", "Si♭ M", "Fa M", "Do M", "Sol M", "Re M", "La M", "Mi M", "Si M", "Fa♯ M", "Do♯ M"];
const LETTER_STEP: Record<string, number> = { c: 0, d: 1, e: 2, f: 3, g: 4, a: 5, b: 6 };

/** Spell a MIDI note number as a pitch (used for drum notes). */
const SHARP_SPELL: [number, number][] = [[0, 0], [0, 1], [1, 0], [1, 1], [2, 0], [3, 0], [3, 1], [4, 0], [4, 1], [5, 0], [5, 1], [6, 0]];
const pitchFromMidi = (midi: number): Pitch => {
  const [step, alter] = SHARP_SPELL[((midi % 12) + 12) % 12];
  return { step, alter, octave: Math.floor(midi / 12) - 1 };
};

export default function App() {
  const [score, setScore] = createSignal<Score | null>(null);
  const [cursor, setCursor] = createSignal<Cursor>({ part: 0, voice: 0, m: 0, start: 0 });
  const [dur, setDur] = createSignal(5);
  const [dotted, setDotted] = createSignal(false);
  const [pendingAlter, setPendingAlter] = createSignal<number | null>(null);
  const [inputMode, setInputMode] = createSignal(false);
  /** Entrada por traste (F2) en una parte con tablatura: cuerda activa (1 = la más aguda) y última nota escrita. */
  const [fretMode, setFretMode] = createSignal(false);
  const [fretString, setFretString] = createSignal(1);
  let lastFret: { part: number; voice: number; m: number; start: number; string: number; fret: number; t: number } | null = null;
  const [lastMidi, setLastMidi] = createSignal(67);
  const [playing, setPlaying] = createSignal<string | null>(null);
  const [isPlaying, setIsPlaying] = createSignal(false);
  const [zoom, setZoom] = createSignal(100);
  const [status, setStatus] = createSignal("Pulsa N para entrar en modo de entrada de notas.");
  const [showNew, setShowNew] = createSignal(false);
  const [audioFile, setAudioFile] = createSignal<string | null>(null);
  /** Reconocimiento óptico (Audiveris): diálogo de instalación y reconocimiento en curso. */
  const [omrSetup, setOmrSetup] = createSignal(false);
  /** Asistente: resultado de «Revisar» (lista de avisos) abierto en un diálogo. */
  const [issues, setIssues] = createSignal<Awaited<ReturnType<typeof api.reviewScore>> | null>(null);
  const [omrBusy, setOmrBusy] = createSignal<string | null>(null);
  const [dirty, setDirty] = createSignal(false);
  const player = new Player();

  // What is drawn: notes under an octave line sit an octave away from where they sound.
  const shown = createMemo(() => (score() ? displayScore(score()!) : null));
  const layout = createMemo(() => (shown() ? layoutScore(shown()!) : null));
  const ticks = () => Math.round(BASE[dur()] * (dotted() ? 1.5 : 1));

  const apply = (s: Score, markDirty = true) => {
    setScore(s);
    if (markDirty) setDirty(true);
    // Keep the cursor on a valid event.
    const c = cursor();
    const part = Math.min(c.part, s.parts.length - 1);
    const m = Math.min(c.m, s.parts[part].measures.length - 1);
    const evs = voiceEvents(s.parts[part].measures[m], c.voice);
    const ev = evs ? (evs.find((e) => e.start <= c.start && c.start < e.start + e.ticks) ?? evs[0]) : null;
    setCursor({ part, voice: c.voice, m, start: ev ? ev.start : 0 });
  };

  /** Editor del símbolo de acorde (clic en un acorde, o K en la posición del cursor). */
  const [chordEdit, setChordEdit] = createSignal<{ measure: number; tick: number; text: string; x: number; y: number } | null>(null);
  const openChordEditor = (measure: number, tick: number, x = 260, y = 230) => {
    const cur = score()?.chords?.find((c) => c.measure === measure && c.tick === tick);
    setChordEdit({ measure, tick, text: cur ? chordName(cur, keyAt(score()!, measure), score()!.parts[0]?.transpose) : "", x, y });
  };
  const commitChord = async (text: string) => {
    const ed = chordEdit();
    if (!ed) return;
    if (await run(api.setChord(ed.measure, ed.tick, text))) {
      setChordEdit(null);
      setStatus(text.trim() ? `Acorde fijado en el compás ${ed.measure + 1}.` : `Acorde del compás ${ed.measure + 1} borrado.`);
    }
  };

  const toggleChords = async () => {
    const has = !!score()?.chords?.length;
    if (await run(has ? api.clearChords() : api.detectChords())) {
      const n = score()?.chords?.length ?? 0;
      setStatus(has ? "Cifrado de acordes quitado." : n ? `${n} acordes detectados (borrador): revísalos; se dibujan sobre el primer pentagrama.` : "No se detectaron acordes (no hay notas suficientes).");
    }
  };

  const refreshChords = async () => {
    const kept = score()?.chords?.filter((c) => c.manual).length ?? 0;
    if (await run(api.detectChords())) {
      const n = score()?.chords?.length ?? 0;
      setStatus(`Acordes actualizados con las notas actuales: ${n} en total${kept ? `, ${kept} fijados a mano conservados` : ""}.`);
    }
  };

  /** Entrada de letra: escribe una sílaba por nota; Espacio avanza, «-» avanza uniendo con guion, Retroceso vuelve atrás. */
  const [lyricEdit, setLyricEdit] = createSignal<{ verse: number; text: string } | null>(null);
  const noteStarts = () => {
    const s = score();
    if (!s) return [];
    const c = cursor();
    return positions().filter((p) => voiceEvents(s.parts[c.part].measures[p.m], c.voice)?.find((e) => e.start === p.start)?.kind === "note");
  };
  const lyricOf = (m: number, start: number, verse: number) =>
    voiceEvents(score()!.parts[cursor().part].measures[m], cursor().voice)?.find((e) => e.start === start)?.lyrics?.find((y) => y.verse === verse);
  const startLyrics = (verse = 0) => {
    const notes = noteStarts();
    if (!notes.length) { setStatus("No hay notas para poner letra."); return; }
    const c = cursor();
    const here = notes.find((p) => p.m === c.m && p.start === c.start) ?? notes.find((p) => p.m > c.m || (p.m === c.m && p.start > c.start)) ?? notes[0];
    setCursor({ ...c, m: here.m, start: here.start });
    setLyricEdit({ verse, text: lyricOf(here.m, here.start, verse)?.text ?? "" });
    setStatus("Letra: escribe la sílaba; Espacio = siguiente nota, «-» = sílaba con guion, «_» = alargar con línea de melisma, Supr (vacío) = quitarla, Retroceso (vacío) = nota anterior, Esc = terminar.");
  };
  const lyricCommit = async (text: string, hyphen: boolean) => {
    const ed = lyricEdit();
    if (!ed) return;
    const c = cursor();
    const notes = noteStarts();
    const i = notes.findIndex((p) => p.m === c.m && p.start === c.start);
    const prev = i > 0 ? lyricOf(notes[i - 1].m, notes[i - 1].start, ed.verse) : undefined;
    const cont = prev?.syllabic === "begin" || prev?.syllabic === "middle";
    const syl = hyphen ? (cont ? "middle" : "begin") : cont ? "end" : "single";
    const t = text.trim();
    if (t || lyricOf(c.m, c.start, ed.verse)) {
      if (!(await run(api.setLyric(c.part, c.voice, c.m, c.start, ed.verse, t, syl)))) return;
    }
    const next = notes[i + 1];
    if (!next) { setLyricEdit(null); setStatus("Letra: fin de las notas."); return; }
    setCursor({ ...c, m: next.m, start: next.start });
    setLyricEdit({ verse: ed.verse, text: lyricOf(next.m, next.start, ed.verse)?.text ?? "" });
  };
  /** «_»: la sílaba escrita (o la anterior, si el campo está vacío) se alarga con una línea de melisma sobre la nota siguiente (o la actual). */
  const lyricUnderscore = async (text: string) => {
    const ed = lyricEdit();
    if (!ed) return;
    const c = cursor();
    const notes = noteStarts();
    const i = notes.findIndex((p) => p.m === c.m && p.start === c.start);
    const t = text.trim();
    let origin = -1;
    let target = i + 1;
    if (t) {
      const prev = i > 0 ? lyricOf(notes[i - 1].m, notes[i - 1].start, ed.verse) : undefined;
      const syl = prev?.syllabic === "begin" || prev?.syllabic === "middle" ? "end" : "single";
      if (!(await run(api.setLyric(c.part, c.voice, c.m, c.start, ed.verse, t, syl)))) return;
      origin = i;
    } else {
      for (let j = i - 1; j >= 0 && origin < 0; j--) if (lyricOf(notes[j].m, notes[j].start, ed.verse)) origin = j;
      target = i;
    }
    if (origin < 0 || target >= notes.length) { setStatus("No hay sílaba que alargar o no hay más notas."); return; }
    const tn = notes[target];
    const tev = voiceEvents(score()!.parts[c.part].measures[tn.m], c.voice)!.find((e) => e.start === tn.start)!;
    const o = notes[origin];
    if (!(await run(api.setLyricExtend(c.part, c.voice, o.m, o.start, ed.verse, tn.m, tn.start + tev.ticks)))) return;
    const next = notes[target + 1];
    if (!next) { setLyricEdit(null); setStatus("Letra: fin de las notas."); return; }
    setCursor({ ...c, m: next.m, start: next.start });
    setLyricEdit({ verse: ed.verse, text: lyricOf(next.m, next.start, ed.verse)?.text ?? "" });
  };
  /** Supr con el campo vacío (o el botón): quita la línea de melisma que pasa por la nota actual o nace en ella. */
  const lyricClearExtend = async () => {
    const ed = lyricEdit();
    if (!ed) return;
    const c = cursor();
    const notes = noteStarts();
    const i = notes.findIndex((p) => p.m === c.m && p.start === c.start);
    for (let j = i; j >= 0; j--) {
      const l = lyricOf(notes[j].m, notes[j].start, ed.verse);
      if (!l?.extend_to) continue;
      const e = l.extend_to;
      // the line reaches the current note when it ends after the note starts
      if (j === i || e.measure > c.m || (e.measure === c.m && e.tick > c.start)) {
        if (await run(api.setLyricExtend(c.part, c.voice, notes[j].m, notes[j].start, ed.verse, null, 0))) setStatus("Línea de melisma quitada.");
        return;
      }
      break;
    }
    setStatus("No hay línea de melisma en esta nota.");
  };
  const lyricBack = () => {
    const ed = lyricEdit();
    const c = cursor();
    const notes = noteStarts();
    const i = notes.findIndex((p) => p.m === c.m && p.start === c.start);
    if (!ed || i <= 0) return;
    const p = notes[i - 1];
    setCursor({ ...c, m: p.m, start: p.start });
    setLyricEdit({ verse: ed.verse, text: lyricOf(p.m, p.start, ed.verse)?.text ?? "" });
  };

  /** Texto de pentagrama (tecla T): texto libre sobre el pentagrama en la posición del cursor. */
  const [textEdit, setTextEdit] = createSignal<{ text: string } | null>(null);
  const openTextEditor = () => {
    const c = cursor();
    const ev = voiceEvents(score()!.parts[c.part].measures[c.m], c.voice)?.find((e) => e.start === c.start);
    if (!ev) { setStatus("Coloca el cursor sobre una nota o un silencio para poner texto."); return; }
    setTextEdit({ text: ev.text ?? "" });
  };
  const commitText = async (text: string) => {
    const c = cursor();
    if (await run(api.setText(c.part, c.voice, c.m, c.start, text))) {
      setTextEdit(null);
      setStatus(text.trim() ? "Texto de pentagrama fijado." : "Texto de pentagrama borrado.");
    }
  };

  const run = async (p: Promise<Score>, markDirty = true) => {
    try {
      apply(await p, markDirty);
      return true;
    } catch (e) {
      setStatus(String(e));
      return false;
    }
  };

  onMount(async () => {
    apply(await api.getScore(), false);
    // Warm up the synthesizer and SoundFont in the background.
    player.init().then(() => setStatus((s) => (s.startsWith("Pulsa N") ? "Sonidos cargados. " + s : s))).catch((e) => setStatus(`Sonido no disponible: ${e}`));
    window.addEventListener("keydown", onKey);
    // `novascore.exe partitura.pdf`: the recognition runs while the window opens; wait for it before anything else
    let omr = await api.omrStartup();
    if (omr.state === "running") {
      setOmrBusy(omr.name ?? "partitura");
      setStatus("Reconociendo la partitura con Audiveris… (puede tardar varios minutos)");
      await new Promise<void>(async (resolve) => {
        const un = await listen("omr-finished", () => {
          un();
          resolve();
        });
        const again = await api.omrStartup(); // it may have finished before we started listening
        if (again.state === "done") {
          un();
          omr = again;
          resolve();
        }
      });
      if (omr.state === "running") omr = await api.omrStartup();
      setOmrBusy(null);
    }
    if (omr.state === "done") {
      if (omr.imported) {
        setCursor({ part: 0, voice: 0, m: 0, start: 0 });
        apply(omr.imported.score, false);
        setDirty(true);
        setStatus(omr.imported.warnings.join(" "));
      } else {
        setStatus(`No se pudo reconocer la partitura: ${omr.error}`);
      }
    }
    // Headless-style export: NOVASCORE_EXPORT_PDF=out.pdf novascore score.musicxml
    const out = await api.startupExport();
    if (out) {
      try {
        await new Promise((r) => setTimeout(r, 400)); // let the music font load
        await document.fonts.load("40px Bravura");
        await api.exportPdf(out, scoreToSvgPages(score()!));
      } finally {
        await api.quit();
      }
    }
  });
  onCleanup(() => {
    window.removeEventListener("keydown", onKey);
    player.stop();
  });

  // ---- cursor movement -------------------------------------------------
  const positions = () => {
    const s = score()!;
    const v = cursor().voice;
    // A measure without this voice still offers one position (start 0) to enter notes.
    return s.parts[cursor().part].measures.flatMap((m, mi) => (voiceEvents(m, v) ?? [{ start: 0 }]).map((e) => ({ m: mi, start: e.start })));
  };
  /** Move the cursor; with Shift the range between the anchor and the cursor is selected. */
  const moveWithSelection = (delta: number, extend: boolean) => {
    if (extend) {
      if (!anchor()) setAnchor(cursor());
    } else {
      setAnchor(null);
    }
    move(delta);
  };
  const move = (delta: number) => {
    const list = positions();
    const c = cursor();
    const i = list.findIndex((p) => p.m === c.m && p.start === c.start);
    const n = list[Math.max(0, Math.min(list.length - 1, i + delta))];
    setCursor({ ...c, ...n });
  };

  // ---- note entry --------------------------------------------------------
  /** Keys and clicks give written pitches; a transposing instrument stores the sounding one. */
  const toSounding = (part: number, p: Pitch): Pitch => soundingPitch(p, score()!.parts[part].transpose);
  const keyFifths = (part: number, m: number) => partKeyAt(score()!, score()!.parts[part], m);
  const pitchFor = (step: number, alterOverride: number | null): Pitch => {
    const alter = alterOverride ?? keyAlter(keyFifths(cursor().part, cursor().m), step);
    let best: Pitch = { step, alter, octave: 4 };
    let bestD = Infinity;
    for (let o = 0; o <= 8; o++) {
      const p = { step, alter, octave: o };
      const d = Math.abs(pitchMidi(toSounding(cursor().part, p)) - lastMidi());
      if (d < bestD) {
        bestD = d;
        best = p;
      }
    }
    return best;
  };

  const [lastEntered, setLastEntered] = createSignal<Cursor | null>(null);

  // ---- range selection (Shift+arrows / Shift+click) and expression marks ----------------
  const [anchor, setAnchor] = createSignal<Cursor | null>(null);
  const selection = createMemo<Selection | null>(() => {
    const a = anchor();
    const c = cursor();
    if (!a || a.part !== c.part || a.voice !== c.voice) return null;
    const lo = a.m * 100000 + a.start <= c.m * 100000 + c.start ? a : c;
    const hi = lo === a ? c : a;
    return { part: c.part, voice: c.voice, from: { m: lo.m, start: lo.start }, to: { m: hi.m, start: hi.start } };
  });

  /** The selected range, or the cursor event alone. */
  const currentRange = () => selection() ?? { part: cursor().part, voice: cursor().voice, from: { m: cursor().m, start: cursor().start }, to: { m: cursor().m, start: cursor().start } };

  const addSpanner = async (kind: SpannerKind) => {
    const r = currentRange();
    const evs = (m: number) => voiceEvents(score()!.parts[r.part].measures[m], r.voice) ?? [];
    const toEv = evs(r.to.m).find((e) => e.start === r.to.start);
    if (!toEv) return;
    let start = { measure: r.from.m, tick: r.from.start };
    let end: { measure: number; tick: number };
    if (kind === "slur") {
      // A slur joins two notes: the last selected one, or the next event after a single one.
      if (r.from.m === r.to.m && r.from.start === r.to.start) {
        const list = positions();
        const i = list.findIndex((p) => p.m === r.to.m && p.start === r.to.start);
        const nxt = list[i + 1];
        if (!nxt) {
          setStatus("No hay una nota siguiente para cerrar la ligadura.");
          return;
        }
        end = { measure: nxt.m, tick: nxt.start };
      } else {
        end = { measure: r.to.m, tick: r.to.start };
      }
    } else {
      end = { measure: r.to.m, tick: toEv.start + toEv.ticks };
      if (end.tick > measureLen(score()!, end.measure)) end.tick = measureLen(score()!, end.measure);
    }
    if (await run(api.toggleSpanner(r.part, kind, r.voice, start, end))) {
      setStatus(
        kind === "slur" ? "Ligadura de expresión (S de nuevo la quita)"
        : kind === "crescendo" ? "Crescendo (< de nuevo lo quita)"
        : kind === "decrescendo" ? "Decrescendo (> de nuevo lo quita)"
        : kind === "pedal" ? "Pedal (púlsalo otra vez sobre lo mismo para quitarlo)"
        : kind === "trill" ? "Trino (suena alternando con la nota superior)"
        : "Octava (púlsala otra vez sobre lo mismo para quitarla): las notas suenan desplazadas",
      );
    }
  };
  /** Dynamic marking at the cursor event; pressing the same one again removes it. */
  const setDynamic = async (name: string) => {
    const c = cursor();
    await run(api.setDynamic(c.part, c.voice, c.m, c.start, name));
  };
  /** Articulation on every note of the selection (or the cursor note). */
  const toggleArtic = async (kind: ArticKind) => {
    const r = currentRange();
    await run(api.toggleArticulation(r.part, r.voice, { measure: r.from.m, tick: r.from.start }, { measure: r.to.m, tick: r.to.start }, kind));
  };
  /** First (mask 1) or second (mask 2) ending over the measures of the selection. */
  const addVolta = async (mask: number) => {
    const r = currentRange();
    const end = { measure: r.to.m, tick: measureLen(score()!, r.to.m) };
    if (await run(api.toggleSpanner(0, "volta", 0, { measure: r.from.m, tick: 0 }, end, mask))) {
      setStatus("Casilla creada: añade la repetición «:|» al final de la primera casilla. Púlsala otra vez para quitarla.");
    }
  };
  const toggleRepeat = async (start: boolean) => {
    await run(api.toggleRepeat(cursor().m, start));
  };
  const toggleMarker = async (marker: Marker) => {
    await run(api.toggleMarker(cursor().m, marker));
  };
  const setJump = async (jump: Jump) => {
    await run(api.setJump(cursor().m, jump));
  };
  const removeSpanners = async () => {
    const r = currentRange();
    const toEv = voiceEvents(score()!.parts[r.part].measures[r.to.m], r.voice)?.find((e) => e.start === r.to.start);
    const end = { measure: r.to.m, tick: (toEv?.start ?? r.to.start) + (toEv?.ticks ?? 0) };
    await run(api.removeSpanners(r.part, r.voice, { measure: r.from.m, tick: r.from.start }, end));
  };

  const enter = (written: Pitch) => enterSounding(toSounding(cursor().part, written));
  const enterSounding = async (p: Pitch): Promise<boolean> => {
    const c = cursor();
    try {
      const r = await api.enterNote(c.part, c.voice, c.m, c.start, ticks(), p);
      setScore(r.score);
      setDirty(true);
      setLastEntered(c);
      setCursor({ ...c, m: r.measure, start: r.start });
      setLastMidi(pitchMidi(p));
      setPendingAlter(null);
      return true;
    } catch (e) {
      setStatus(String(e));
      return false;
    }
  };

  // ---- entrada por traste ---------------------------------------------------
  const curTech = (): Tech | undefined => {
    const t = targetNote();
    return t ? voiceEvents(score()!.parts[t.part].measures[t.m], t.voice)?.find((x) => x.start === t.start)?.tech : undefined;
  };
  const fretTuning = () => score()?.parts[cursor().part]?.tuning ?? null;
  const toggleFretMode = () => {
    if (fretMode()) {
      setFretMode(false);
      setStatus("Entrada por traste desactivada.");
      return;
    }
    const t = fretTuning();
    if (!t) {
      setStatus("Entrada por traste: elige antes «Tablatura» en el selector del instrumento.");
      return;
    }
    setFretMode(true);
    setInputMode(true);
    setFretString(Math.min(fretString(), t.length));
    setStatus(`Entrada por traste: ↑↓ cuerda (${fretString()}), 0–9 traste (dos cifras seguidas = traste de dos cifras), [ ] duración, R silencio, F2 sale.`);
  };
  /** Digit typed in fret mode: writes the note at the active string, or extends the last fret to two digits. */
  const typeFret = async (digit: number) => {
    const t = fretTuning();
    if (!t) return;
    const n = t.length;
    const c = cursor();
    const now = Date.now();
    const last = lastFret;
    // second digit of a two-digit fret: change the note just written
    if (last && last.part === c.part && last.voice === c.voice && now - last.t < 900 && last.fret * 10 + digit <= MAX_FRET && last.fret >= 1) {
      const fret = last.fret * 10 + digit;
      if (await run(api.transpose(last.part, last.voice, last.m, last.start, fret - last.fret))) {
        await run(api.setString(last.part, last.voice, last.m, last.start, t[n - last.string] + fret, last.string));
        lastFret = { ...last, fret, t: now };
        setCursor({ ...c });
        setStatus(`Cuerda ${last.string}, traste ${fret}`);
      }
      return;
    }
    const string = fretString();
    const midi = t[n - string] + digit;
    const start = c.start;
    const m = c.m;
    if (await enterSounding(pitchFromMidi(midi))) {
      await run(api.setString(c.part, c.voice, m, start, midi, string), false);
      lastFret = { part: c.part, voice: c.voice, m, start, string, fret: digit, t: now };
      setStatus(`Cuerda ${string}, traste ${digit}`);
    }
  };
  const fretKey = async (e: KeyboardEvent, k: string): Promise<boolean> => {
    const t = fretTuning();
    if (!fretMode() || !t) return false;
    if (k >= "0" && k <= "9" && k.length === 1) {
      await typeFret(Number(k));
    } else if (k === "arrowup" || k === "arrowdown") {
      setFretString(Math.max(1, Math.min(t.length, fretString() + (k === "arrowup" ? -1 : 1))));
      lastFret = null;
      setStatus(`Cuerda ${fretString()}`);
    } else if (k === "[") {
      setDur(Math.max(3, dur() - 1));
    } else if (k === "]") {
      setDur(Math.min(7, dur() + 1));
    } else if (k === "r") {
      lastFret = null;
      await enterRest();
    } else if (k === "arrowleft" || k === "arrowright" || k === "backspace" || k === "delete") {
      lastFret = null;
      return false;
    } else return false;
    void e;
    return true;
  };

  /** Guitar technique on the note under the cursor (the same one again removes it). */
  const setTech = async (tech: Tech | null) => {
    const t = targetNote();
    if (!t) {
      setStatus("Elige primero una nota de la tablatura.");
      return;
    }
    await run(api.setTech(t.part, t.voice, t.m, t.start, tech));
  };

  /** The note that chord/tie commands act on: the selected note, else the last one entered. */
  const targetNote = (): Cursor | null => {
    const s = score()!;
    const isNote = (c: Cursor) => (s.parts[c.part]?.measures[c.m] ? voiceEvents(s.parts[c.part].measures[c.m], c.voice) : null)?.find((e) => e.start === c.start)?.kind === "note";
    if (isNote(cursor())) return cursor();
    const l = lastEntered();
    return l && isNote(l) ? l : null;
  };

  /** Shift+letter: add a pitch above the target note's top pitch. */
  const addToChord = async (step: number) => {
    const t = targetNote();
    if (!t) return;
    const ev = voiceEvents(score()!.parts[t.part].measures[t.m], t.voice)?.find((e) => e.start === t.start);
    if (ev?.kind !== "note") return;
    const top = Math.max(...ev.pitches.map(pitchMidi));
    const alter = pendingAlter() ?? keyAlter(keyFifths(t.part, t.m), step);
    let p: Pitch | null = null;
    for (let o = 0; o <= 8 && !p; o++) {
      const cand = toSounding(t.part, { step, alter, octave: o });
      if (pitchMidi(cand) > top) p = cand;
    }
    if (!p) return;
    if (await run(api.addPitch(t.part, t.voice, t.m, t.start, p))) {
      setPendingAlter(null);
      setLastMidi(pitchMidi(p));
    }
  };

  /** Clic en un número de la tablatura: pasa esa nota a la siguiente cuerda en la que se puede tocar. */
  const cycleString = async (part: number, voice: number, m: number, start: number, midi: number) => {
    const s = score()!;
    const tuning = s.parts[part].tuning;
    if (!tuning) return;
    const ev = voiceEvents(s.parts[part].measures[m], voice)?.find((e) => e.start === start);
    if (ev?.kind !== "note") return;
    const idx = ev.pitches.findIndex((p) => pitchMidi(p) === midi);
    const current = tabPositions(s, part, voice).get(`${m}:${start}`)?.[idx]?.string ?? 0;
    const options = playableStrings(tuning, midi);
    if (!options.length) { setStatus("Esa nota no se puede tocar con esta afinación."); return; }
    // the first click on a note selects it; the next ones move it to another string
    const c = cursor();
    if (c.part !== part || c.voice !== voice || c.m !== m || c.start !== start) {
      setCursor({ part, voice, m, start });
      return;
    }
    const next = options[(options.indexOf(current) + 1) % options.length];
    if (await run(api.setString(part, voice, m, start, midi, next))) setStatus(`Nota movida a la cuerda ${next} (se conserva al guardar).`);
  };

  const [newInstrument, setNewInstrument] = createSignal(6);
  const instrumentIndex = (program: number, clef: string, percussion: boolean, tr?: { diatonic: number; chromatic: number }) => {
    if (percussion) return INSTRUMENTS.findIndex((i) => i.percussion);
    const sameTr = (i: (typeof INSTRUMENTS)[number]) => (i.transpose?.chromatic ?? 0) === (tr?.chromatic ?? 0) && (i.transpose?.diatonic ?? 0) === (tr?.diatonic ?? 0);
    const exact = INSTRUMENTS.findIndex((i) => !i.percussion && i.program === program && i.clef === clef && sameTr(i));
    if (exact >= 0) return exact;
    const sameProgram = INSTRUMENTS.findIndex((i) => !i.percussion && i.program === program && i.clef === clef);
    return sameProgram >= 0 ? sameProgram : INSTRUMENTS.findIndex((i) => !i.percussion && i.program === program);
  };
  const selectPart = (part: number) => {
    const c = cursor();
    const evs = voiceEvents(score()!.parts[part].measures[c.m], c.voice);
    const ev = evs ? (evs.find((e) => e.start <= c.start && c.start < e.start + e.ticks) ?? evs[0]) : null;
    setCursor({ ...c, part, start: ev ? ev.start : 0 });
  };
  const addInstrument = async () => {
    const ins = INSTRUMENTS[newInstrument()];
    if (await run(api.addPart(ins.name, ins.clef, ins.program, ins.percussion ?? false, ins.transpose, ins.tuning))) selectPart(score()!.parts.length - 1);
  };

  /** Switch the active voice (1-4), keeping the cursor on the same beat when possible. */
  const setVoice = (v: number) => {
    const c = cursor();
    const evs = voiceEvents(score()!.parts[c.part].measures[c.m], v);
    const ev = evs?.find((e) => e.start <= c.start && c.start < e.start + e.ticks) ?? evs?.[0];
    setCursor({ ...c, voice: v, start: ev ? ev.start : 0 });
    setStatus(`Voz ${v + 1}`);
  };

  const toggleTie = async () => {
    const t = targetNote();
    if (t) await run(api.toggleTie(t.part, t.voice, t.m, t.start));
  };

  const enterRest = async () => {
    const c = cursor();
    try {
      const r = await api.enterRest(c.part, c.voice, c.m, c.start, ticks());
      setScore(r.score);
      setDirty(true);
      setCursor({ ...c, m: r.measure, start: r.start });
    } catch (e) {
      setStatus(String(e));
    }
  };

  /** Turn the selected note or rest into a tuplet (3:2 triplet by default). */
  const makeTuplet = async (actual: number, normal: number) => {
    const c = cursor();
    const ev = voiceEvents(score()!.parts[c.part].measures[c.m], c.voice)?.find((x) => x.start === c.start);
    if (!ev) return;
    if (await run(api.makeTuplet(c.part, c.voice, c.m, c.start, actual, normal))) {
      // Typing now fills the slots: select the written value of one slot.
      const slot = (ev.ticks / actual) * actual / normal;
      const key = Object.entries(BASE).find(([, v]) => v === slot)?.[0];
      if (key) {
        setDur(Number(key));
        setDotted(false);
      }
      setStatus(`Tuplet ${actual}:${normal} — escribe las notas de cada hueco (o Ctrl+0 para quitarlo)`);
    }
  };
  const clearTuplet = async () => {
    const c = cursor();
    await run(api.clearTuplet(c.part, c.voice, c.m, c.start));
  };

  const onPick = async (part: number, voice: number, m: number, start: number, pos: number | null, extend = false) => {
    const c = cursor();
    if (extend && c.part === part && c.voice === voice) {
      if (!anchor()) setAnchor(c);
      setCursor({ part, voice, m, start });
      return;
    }
    setAnchor(null);
    setCursor({ part, voice, m, start });
    if (inputMode() && pos !== null) {
      const s = score()!;
      if (s.parts[part].percussion) {
        await enter(pitchFromMidi(drumFromPos(pos)));
        return;
      }
      const bottom = s.parts[part].clef === "treble" ? 30 : 18;
      const idx = bottom + pos;
      const step = ((idx % 7) + 7) % 7;
      // A click is a written position: under an octave line the note sounds shifted.
      const p: Pitch = { step, alter: pendingAlter() ?? keyAlter(keyFifths(part, m), step), octave: Math.floor(idx / 7) + ottavaShiftAt(s.parts[part], m, start) };
      await enter(p);
    }
  };

  // ---- file / playback -------------------------------------------------
  const doSave = async () => {
    const path = await save({ filters: [{ name: "NovaScore", extensions: ["nova"] }], defaultPath: `${score()!.title}.nova` });
    if (!path) return;
    try {
      await api.save(path);
      setDirty(false);
      setStatus(`Guardado: ${path}`);
    } catch (e) {
      setStatus(String(e));
    }
  };
  const doOpen = async () => {
    const path = await open({ filters: [{ name: "NovaScore", extensions: ["nova"] }], multiple: false });
    if (typeof path !== "string") return;
    setCursor({ part: 0, voice: 0, m: 0, start: 0 });
    if (await run(api.load(path), false)) {
      setDirty(false);
      setStatus(`Abierto: ${path}`);
    }
  };
  const doExportMuseScore = async () => {
    const path = await save({ filters: [{ name: "MuseScore", extensions: ["mscz", "mscx"] }], defaultPath: `${score()!.title}.mscz` });
    if (!path) return;
    try {
      await api.exportMscz(path);
      setStatus(`Guardado como MuseScore: ${path}`);
    } catch (e) {
      setStatus(`No se pudo guardar como MuseScore: ${e}`);
    }
  };
  const doExport = async (kind: "midi" | "xml") => {
    const ext = kind === "midi" ? "mid" : "musicxml";
    const path = await save({ filters: [{ name: kind === "midi" ? "MIDI" : "MusicXML", extensions: [ext] }], defaultPath: `${score()!.title}.${ext}` });
    if (!path) return;
    try {
      await (kind === "midi" ? api.exportMidi(path) : api.exportMusicxml(path));
      setStatus(`Exportado: ${path}`);
    } catch (e) {
      setStatus(String(e));
    }
  };
  const doExportPdf = async () => {
    const path = await save({ filters: [{ name: "PDF", extensions: ["pdf"] }], defaultPath: `${score()!.title}.pdf` });
    if (!path) return;
    try {
      setStatus("Generando PDF…");
      await api.exportPdf(path, scoreToSvgPages(score()!));
      setStatus(`Exportado: ${path}`);
    } catch (e) {
      setStatus(`No se pudo exportar el PDF: ${e}`);
    }
  };
  const doImport = async (kind: "xml" | "midi" | "mscz") => {
    const filters = {
      xml: [{ name: "MusicXML", extensions: ["musicxml", "xml", "mxl"] }],
      midi: [{ name: "MIDI", extensions: ["mid", "midi"] }],
      mscz: [{ name: "MuseScore", extensions: ["mscz", "mscx"] }],
    }[kind];
    const path = await open({ filters, multiple: false });
    if (typeof path !== "string") return;
    try {
      setStatus("Importando…");
      const r = await (kind === "xml" ? api.importMusicxml(path) : kind === "midi" ? api.importMidi(path) : api.importMscz(path));
      setCursor({ part: 0, voice: 0, m: 0, start: 0 });
      apply(r.score, false);
      setDirty(true);
      setStatus(r.warnings.length ? `Importado. ${r.warnings.join(" ")}` : `Importado: ${path}`);
    } catch (e) {
      setStatus(String(e));
    }
  };
  const doAccompaniment = async (style: "bass" | "blocks" | "arpeggio") => {
    try {
      const r = await api.addAccompaniment(style);
      apply(r.score, true);
      selectPart(r.part);
      setDirty(true);
      const detected = score()!.chords?.some((c) => !c.manual);
      setStatus(`Acompañamiento «${r.score.parts[r.part].name}» añadido siguiendo el cifrado de acordes${detected ? " (detectado: revísalo con las teclas K o «Actualizar acordes»)" : ""}. Se puede deshacer con Ctrl+Z.`);
    } catch (e) {
      setStatus(String(e));
    }
  };
  /** Asistente: continúa la parte del cursor como secuencia (`phrase` compases, `steps` grados) o con una cadencia final (`phrase` = 0). */
  const doContinue = async (phrase: number, steps: number, kind: "degrees" | "semitones" | "invert" = "degrees") => {
    const part = cursor().part;
    try {
      const r = await api.continuePhrase(part, phrase, steps, kind);
      apply(r.score, true);
      setCursor({ part, voice: 0, m: r.part, start: 0 });
      setDirty(true);
      const bars = `${phrase} ${phrase > 1 ? "compases" : "compás"}`;
      if (phrase > 0 && kind === "semitones") {
        setStatus(`Modulación añadida (${bars}, ${steps > 0 ? "+" : ""}${steps} semitonos). La armadura cambia donde empieza. Ctrl+Z la quita.`);
        return;
      }
      if (phrase > 0 && kind === "invert") {
        setStatus(`Inversión añadida (${bars}, en espejo por la escala alrededor de la primera nota de la frase). Ctrl+Z la quita.`);
        return;
      }
      setStatus(phrase === 0 ? "Compás final con la tónica añadido. Se puede deshacer con Ctrl+Z." : `Secuencia añadida (${bars}, ${Math.abs(steps)} grado${Math.abs(steps) > 1 ? "s" : ""} ${steps > 0 ? "más arriba" : "más abajo"}). Repite para alargarla; Ctrl+Z la quita.`);
    } catch (e) {
      setStatus(String(e));
    }
  };
  /** Asistente: escribe el cifrado de acordes de la parte del cursor (analizando su melodía) o una progresión de números romanos. */
  const doHarmonize = async (minor: boolean, progression?: string, variant = 0) => {
    const perBar = chordsPerBar();
    try {
      const r = await api.harmonize(cursor().part, minor, progression, perBar, { sevenths: sevenths(), inversions: inversions(), secondary: secondary(), variant });
      setLastHarm({ minor, progression });
      setHarmVariant(variant);
      apply(r.score, true);
      setDirty(true);
      setStatus(`${progression ? `Progresión ${progression}` : `Armonización de la melodía (${minor ? "menor" : "mayor"}${variant > 0 ? `, propuesta ${variant + 1}` : ""})`}: ${r.part} símbolo${r.part > 1 ? "s" : ""} de acorde, ${perBar === 4 ? "hasta uno por pulso (un pulso sin nota nueva se une al anterior)" : perBar === 2 ? "hasta dos por compás (se parte el compás solo donde la música se mueve en su segunda mitad; los compases de 3 tiempos no se parten)" : "uno por compás"}. Pide «Acompañamiento» para tocarlos; Ctrl+Z los quita.`);
    } catch (e) {
      setStatus(String(e));
    }
  };
  /** Armonizar: acordes por compás (1 o 2). */
  const [chordsPerBar, setChordsPerBar] = createSignal(1);
  const [sevenths, setSevenths] = createSignal(false);
  const [harmVariant, setHarmVariant] = createSignal(0);
  const [lastHarm, setLastHarm] = createSignal<{ minor: boolean; progression?: string } | null>(null);
  const [inversions, setInversions] = createSignal(false);
  const [secondary, setSecondary] = createSignal(false);
  const doReview = async () => {
    const list = await api.reviewScore();
    if (!list.length) {
      setStatus("Revisión: no se encontró nada raro (tesitura, voces cruzadas, quintas y octavas paralelas o directas, saltos grandes).");
      return;
    }
    setIssues(list);
  };
  const startOmr = async () => {
    const st = await api.omrStatus();
    if (!st.found) {
      setOmrSetup(true);
      return;
    }
    const path = await open({ filters: [{ name: "Partitura impresa (PDF o imagen)", extensions: ["pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif"] }], multiple: false });
    if (typeof path !== "string") return;
    const name = path.split(/[\/]/).pop() ?? path;
    setOmrBusy(name);
    setStatus("Reconociendo la partitura con Audiveris… (puede tardar varios minutos)");
    try {
      const r = await api.importOmr(path);
      setCursor({ part: 0, voice: 0, m: 0, start: 0 });
      apply(r.score, false);
      setDirty(true);
      setStatus(r.warnings.join(" "));
    } catch (e) {
      setStatus(`No se pudo reconocer la partitura: ${e}`);
    } finally {
      setOmrBusy(null);
    }
  };
  const chooseAudiveris = async () => {
    const path = await open({ multiple: false, title: "Ejecutable de Audiveris" });
    if (typeof path !== "string") return;
    try {
      const st = await api.setOmrPath(path);
      if (st.found) {
        setOmrSetup(false);
        setStatus(`Audiveris configurado: ${st.path}. Pulsa de nuevo «Importar partitura impresa».`);
      }
    } catch (e) {
      setStatus(String(e));
    }
  };
  const pickAudio = async () => {
    const path = await open({ filters: [{ name: "Audio", extensions: ["wav", "mp3", "flac", "ogg", "oga", "m4a", "aac"] }], multiple: false });
    if (typeof path === "string") setAudioFile(path);
  };
  const doImportAudio = async (path: string, options: AudioOptions) => {
    setAudioFile(null);
    try {
      setStatus("Transcribiendo audio… (puede tardar un poco)");
      const r = await api.importAudio(path, options);
      setCursor({ part: 0, voice: 0, m: 0, start: 0 });
      apply(r.score, false);
      setDirty(true);
      setStatus(r.warnings.join(" "));
    } catch (e) {
      setStatus(`No se pudo transcribir el audio: ${e}`);
    }
  };
  const togglePlay = async () => {
    if (isPlaying()) {
      player.stop();
      setIsPlaying(false);
      setPlaying(null);
      return;
    }
    setIsPlaying(true);
    try {
      const sounds = await api.getSounds();
      await player.play(score()!, sounds, cursor().m, setPlaying, () => {
        setIsPlaying(false);
        setPlaying(null);
      });
    } catch (e) {
      setIsPlaying(false);
      setStatus(`No se pudo reproducir: ${e}`);
    }
  };

  // ---- keyboard ----------------------------------------------------------
  const onKey = async (e: KeyboardEvent) => {
    if (!score() || showNew() || audioFile() || omrSetup() || omrBusy() || issues()) return;
    const tag = (e.target as HTMLElement).tagName;
    if (tag === "INPUT" || tag === "SELECT") return;
    const k = e.key.toLowerCase();
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.altKey && k >= "1" && k <= "4") {
      setVoice(Number(k) - 1);
      e.preventDefault();
      return;
    }
    if (mod) {
      if (k === "z" && !e.shiftKey) await run(api.undo());
      else if (k === "y" || (k === "z" && e.shiftKey)) await run(api.redo());
      else if (k === "s") await doSave();
      else if (k === "o") await doOpen();
      else if (k === "n") setShowNew(true);
      else if (k === "3") await makeTuplet(3, 2);
      else if (k === "5") await makeTuplet(5, 4);
      else if (k === "6") await makeTuplet(6, 4);
      else if (k === "7") await makeTuplet(7, 4);
      else if (k === "0") await clearTuplet();
      else if (e.key === "ArrowUp") await transpose(12);
      else if (e.key === "ArrowDown") await transpose(-12);
      else return;
      e.preventDefault();
      return;
    }
    if (e.altKey) return;
    e.preventDefault();
    if (k === "f2") {
      toggleFretMode();
      return;
    }
    if (await fretKey(e, k)) return;
    if (k === "n") {
      setInputMode(!inputMode());
      setStatus(inputMode() ? "Modo entrada: A–G notas, Mayús+letra acorde, T ligadura, 0 silencio, 3–7 duración, . puntillo, + − alteraciones." : "Modo selección.");
    } else if (k === "escape") setInputMode(false);
    else if (k === " ") togglePlay();
    else if (k >= "3" && k <= "7") setDur(Number(k));
    else if (k === ".") setDotted(!dotted());
    else if (k === "+" || k === "=") setPendingAlter(pendingAlter() === 1 ? null : 1);
    else if (k === "-") setPendingAlter(pendingAlter() === -1 ? null : -1);
    else if (k === "0") await enterRest();
    else if (k === "arrowright") moveWithSelection(1, e.shiftKey);
    else if (k === "arrowleft") moveWithSelection(-1, e.shiftKey);
    else if (k === "s" && e.shiftKey) await toggleArtic("staccato");
    else if (k === "t" && e.shiftKey) await toggleArtic("tenuto");
    else if (k === "m" && e.shiftKey) await toggleArtic("marcato");
    else if (k === "s") await addSpanner("slur");
    else if (k === "<") await addSpanner("crescendo");
    else if (k === ">") await addSpanner("decrescendo");
    else if (k === "arrowup") await transpose(1);
    else if (k === "arrowdown") await transpose(-1);
    else if (k === "k") openChordEditor(cursor().m, cursor().start);
    else if (k === "l") { e.preventDefault(); startLyrics(); }
    else if (k === "t") { e.preventDefault(); openTextEditor(); }
    else if (k === "delete") await run(api.deleteEvent(cursor().part, cursor().voice, cursor().m, cursor().start));
    else if (k === "backspace") {
      await run(api.deleteEvent(cursor().part, cursor().voice, cursor().m, cursor().start));
      move(-1);
    } else if (k === "t") await toggleTie();
    else if (k in LETTER_STEP && e.shiftKey) await addToChord(LETTER_STEP[k]);
    else if (inputMode() && score()!.parts[cursor().part].percussion && k in DRUM_KEYS) await enter(pitchFromMidi(DRUM_KEYS[k]));
    else if (inputMode() && k in LETTER_STEP) await enter(pitchFor(LETTER_STEP[k], pendingAlter()));
  };

  /** Transponer: toda la pieza o desde el compás del cursor hasta el final, con armadura y cifrado. */
  const [transposeScope, setTransposeScope] = createSignal<"all" | "from">("all");
  const doTransposeRange = async (semi: number) => {
    const s = score()!;
    const last = Math.max(...s.parts.map((p) => p.measures.length)) - 1;
    const from = transposeScope() === "all" ? 0 : cursor().m;
    if (await run(api.transposeRange(from, last, semi))) {
      const f = keyAt(score()!, last);
      setStatus(`Transpuesto ${semi > 0 ? "+" : ""}${semi} semitono${Math.abs(semi) > 1 ? "s" : ""} ${from === 0 ? "en toda la pieza" : `desde el compás ${from + 1}`}: armadura y cifrado incluidos (tonalidad final ${KEY_NAMES[f + 7] ?? f}). Ctrl+Z lo deshace.`);
    }
  };
  const transpose = async (semi: number) => {
    const c = cursor();
    const ev = voiceEvents(score()!.parts[c.part].measures[c.m], c.voice)?.find((x) => x.start === c.start);
    if (ev?.kind !== "note") return;
    if (await run(api.transpose(c.part, c.voice, c.m, c.start, semi))) {
      const e2 = voiceEvents(score()!.parts[c.part].measures[c.m], c.voice)?.find((x) => x.start === c.start);
      if (e2?.kind === "note") setLastMidi(pitchMidi(e2.pitches[e2.pitches.length - 1]));
    }
  };

  return (
    <div class="app">
      <header class="topbar">
        <div class="brand">Nova<span>Score</span></div>
        <button onClick={() => setShowNew(true)} title="Nuevo (Ctrl+N)">Nuevo</button>
        <button onClick={doOpen} title="Abrir (Ctrl+O)">Abrir</button>
        <button onClick={doSave} title="Guardar (Ctrl+S)">Guardar{dirty() ? " •" : ""}</button>
        <span class="sep" />
        <button onClick={() => doImport("xml")} title="Importar MusicXML">Importar XML</button>
        <button onClick={() => doImport("mscz")} title="Importar MuseScore (.mscz / .mscx)">Importar MuseScore</button>
        <button onClick={() => doImport("midi")} title="Importar MIDI">Importar MIDI</button>
        <button onClick={startOmr} title="Reconocer una partitura impresa (PDF o imagen) con Audiveris">Importar partitura impresa</button>
        <button onClick={pickAudio} title="Transcribir un archivo de audio (wav, mp3, flac, ogg, m4a) a partitura">Importar audio</button>
        <button onClick={doExportMuseScore} title="Guardar como MuseScore 4 (.mscz)">Exportar MuseScore</button>
        <button onClick={() => doExport("xml")} title="Exportar MusicXML">Exportar XML</button>
        <button onClick={() => doExport("midi")} title="Exportar MIDI">Exportar MIDI</button>
        <button onClick={doExportPdf} title="Exportar PDF">Exportar PDF</button>
        <span class="sep" />
        <button onClick={() => run(api.undo())} title="Deshacer (Ctrl+Z)">↶</button>
        <button onClick={() => run(api.redo())} title="Rehacer (Ctrl+Y)">↷</button>
        <Show when={score()}>
          <button onClick={toggleChords} title="Detectar el cifrado de acordes a partir de las notas (borrador); si ya hay, lo quita">{score()!.chords?.length ? "Quitar acordes" : "Detectar acordes"}</button>
          <Show when={score()!.chords?.length}>
            <button onClick={refreshChords} title="Vuelve a detectar los acordes con las notas actuales; los que fijaste a mano (o leíste de un archivo) se conservan">Actualizar acordes</button>
          </Show>
        </Show>
        <span class="sep" />
        <Show when={score()}>
          <select
            class="assist-select"
            title="Asistente: escribe una parte de acompañamiento a partir del cifrado de acordes (si no hay, se detecta)"
            value=""
            onChange={(e) => { const v = e.currentTarget.value as "bass" | "blocks" | "arpeggio" | ""; e.currentTarget.value = ""; if (v) void doAccompaniment(v); }}
          >
            <option value="">Acompañamiento…</option>
            <option value="bass">Bajo (fundamentales)</option>
            <option value="blocks">Acordes sostenidos</option>
            <option value="arpeggio">Arpegio en corcheas</option>
          </select>
          <select
            class="assist-select"
            title="Asistente: alarga la melodía de la parte seleccionada repitiendo sus últimos compases como secuencia (por grados de la escala) o la cierra con la tónica"
            value=""
            onChange={(e) => { const v = e.currentTarget.value; e.currentTarget.value = ""; if (v) { const [ph, st, kind] = v.split(":"); void doContinue(Number(ph), Number(st), (kind || "degrees") as "degrees" | "semitones" | "invert"); } }}
          >
            <option value="">Continuar frase…</option>
            <option value="1:-1">Secuencia descendente (último compás)</option>
            <option value="2:-1">Secuencia descendente (2 compases)</option>
            <option value="4:-1">Secuencia descendente (4 compases)</option>
            <option value="1:1">Secuencia ascendente (último compás)</option>
            <option value="2:1">Secuencia ascendente (2 compases)</option>
            <option value="4:1">Secuencia ascendente (4 compases)</option>
            <option value="2:7:semitones">Modular a la dominante (+7, 2 compases)</option>
            <option value="2:5:semitones">Modular a la subdominante (+5, 2 compases)</option>
            <option value="2:2:semitones">Modular un tono más arriba (+2, 2 compases)</option>
            <option value="2:1:invert">Inversión en espejo (2 compases)</option>
            <option value="0:0">Terminar con la tónica</option>
          </select>
          <select class="assist-select" title="Cuántos acordes escribe «Armonizar…» por compás. Con dos, el compás solo se parte donde la melodía se mueve en su segunda mitad (3/4 nunca se parte)" value={String(chordsPerBar())} onChange={(e) => setChordsPerBar(Number(e.currentTarget.value))}>
            <option value="1">1 acorde/compás</option>
            <option value="2">2 acordes/compás</option>
            <option value="4">1 acorde/pulso</option>
          </select>
          <select class="assist-select" title="«Armonizar…» con tríadas, o también con séptimas: la de dominante (V7) se favorece y las demás salen cuando la melodía tiene su séptima; el primer y el último acorde siguen siendo tríadas. No afecta a las progresiones, que llevan lo que escribas (V7, ii7…)" value={sevenths() ? "7" : "3"} onChange={(e) => setSevenths(e.currentTarget.value === "7")}>
            <option value="3">Tríadas</option>
            <option value="7">Con séptimas</option>
          </select>
          <label class="field" title="«Armonizar…» puede usar primeras inversiones cuando hacen que el bajo se mueva por grados conjuntos (nunca en el primer ni el último acorde)"><input type="checkbox" checked={inversions()} onChange={(e) => setInversions(e.currentTarget.checked)} /> Inversiones</label>
          <button disabled={!lastHarm() || !!lastHarm()!.progression} title="Otra armonización válida de la misma melodía con las mismas opciones (la propuesta 1 es la mejor que se encontró; el primer y el último acorde no cambian). Repite para ver más; Ctrl+Z vuelve a la anterior" onClick={() => { const l = lastHarm(); if (l) void doHarmonize(l.minor, l.progression, harmVariant() + 1); }}>Otra armonización</button>
          <label class="field" title="«Armonizar…» puede usar dominantes de otros grados (V/vi, V/V…) donde la melodía tiene su sensible; siempre resuelven al grado que anuncian"><input type="checkbox" checked={secondary()} onChange={(e) => setSecondary(e.currentTarget.checked)} /> Dom. secundarias</label>
          <select
            class="assist-select"
            title="Asistente: escribe el cifrado de acordes, un acorde por compás, analizando la melodía de la parte seleccionada o con una progresión clásica (números romanos). Después, «Acompañamiento» lo toca"
            value=""
            onChange={(e) => { const v = e.currentTarget.value; e.currentTarget.value = ""; if (v) { const [mode, prog] = v.split("|"); void doHarmonize(mode === "m", prog || undefined); } }}
          >
            <option value="">Armonizar…</option>
            <option value="M|">Melodía en mayor</option>
            <option value="m|">Melodía en menor</option>
            <option value="M|I-IV-V-I">I–IV–V–I</option>
            <option value="M|I-V-vi-IV">I–V–vi–IV</option>
            <option value="M|I-vi-IV-V">I–vi–IV–V (años 50)</option>
            <option value="M|ii-V-I">ii–V–I</option>
            <option value="M|I-V-vi-iii-IV-I-IV-V">Canon de Pachelbel</option>
            <option value="M|I7-I7-I7-I7-IV7-IV7-I7-I7-V7-IV7-I7-V7">Blues de 12 compases</option>
            <option value="m|i-iv-V-i">i–iv–V–i (menor)</option>
            <option value="m|i-bVII-bVI-V">Cadencia andaluza (menor)</option>
            <option value="m|i-bVI-bIII-bVII">i–VI–III–VII (menor)</option>
          </select>
          <button onClick={doReview} title="Busca notas fuera de la tesitura del instrumento, voces cruzadas, quintas u octavas paralelas o directas y saltos melódicos de más de una octava">Revisar</button>
        </Show>
        <span class="sep" />
        <button class="play" classList={{ on: isPlaying() }} onClick={togglePlay} title="Reproducir (Espacio)">
          {isPlaying() ? "■ Parar" : "▶ Reproducir"}
        </button>
        <Show when={score()}>
          <label class="field">♩ =
            <input type="number" min="20" max="300" value={score()!.tempo} onChange={(e) => run(api.setTempo(Number(e.currentTarget.value)))} />
          </label>
          <label class="field">Compás
            <select title="Compás desde el compás seleccionado hasta el siguiente cambio. Si hay notas en esos compases con otra duración se vacían." value={`${timeAt(score()!, cursor().m).num}/${timeAt(score()!, cursor().m).den}`} onChange={(e) => { const [n, d] = e.currentTarget.value.split("/").map(Number); run(api.setTime(cursor().m, n, d)); }}>
              <For each={["2/4", "3/4", "4/4", "5/4", "6/8", "9/8", "12/8", "2/2", "3/8"]}>{(t) => <option value={t}>{t}</option>}</For>
            </select>
          </label>
          <label class="field">Tonalidad
            <select title="Tonalidad desde el compás seleccionado hasta el siguiente cambio (en el primer compás, la de toda la pieza). Elegir la que ya rige quita el cambio. Las notas no se mueven: solo cambian la armadura y las alteraciones que se dibujan." value={keyAt(score()!, cursor().m)} onChange={(e) => run(api.setKey(cursor().m, Number(e.currentTarget.value)))}>
              <For each={KEY_NAMES}>{(n, i) => <option value={i() - 7}>{n}</option>}</For>
            </select>
          </label>
        </Show>
        <span class="grow" />
        <button onClick={() => setZoom(Math.max(50, zoom() - 10))}>−</button>
        <span class="zoom">{zoom()}%</span>
        <button onClick={() => setZoom(Math.min(200, zoom() + 10))}>+</button>
      </header>

      <div class="toolbar">
        <button classList={{ on: inputMode() }} onClick={() => setInputMode(!inputMode())} title="Modo entrada (N)">✎ Entrada de notas</button>
        <span class="sep" />
        <For each={[3, 4, 5, 6, 7]}>
          {(d) => (
            <button class="glyph" classList={{ on: dur() === d }} onClick={() => setDur(d)} title={`Duración (${d})`}>{DUR_LABEL[d]}</button>
          )}
        </For>
        <button classList={{ on: dotted() }} onClick={() => setDotted(!dotted())} title="Puntillo (.)">·</button>
        <span class="sep" />
        <button classList={{ on: pendingAlter() === 1 }} onClick={() => setPendingAlter(pendingAlter() === 1 ? null : 1)} title="Sostenido (+)">♯</button>
        <button classList={{ on: pendingAlter() === 0 }} onClick={() => setPendingAlter(pendingAlter() === 0 ? null : 0)} title="Becuadro">♮</button>
        <button classList={{ on: pendingAlter() === -1 }} onClick={() => setPendingAlter(pendingAlter() === -1 ? null : -1)} title="Bemol (−)">♭</button>
        <span class="sep" />
        <For each={[0, 1, 2, 3]}>
          {(v) => (
            <button class={`voice v${v}`} classList={{ on: cursor().voice === v }} onClick={() => setVoice(v)} title={`Voz ${v + 1} (Ctrl+Alt+${v + 1})`}>{v + 1}</button>
          )}
        </For>
        <span class="sep" />
        <button onClick={enterRest} title="Silencio (0)">Silencio</button>
        <select class="tuplet-select" value="" title="Convertir la selección en un grupo especial (tresillo: Ctrl+3)" onChange={(e) => { const v = e.currentTarget.value; e.currentTarget.value = ""; if (v === "clear") clearTuplet(); else if (v) { const [a, n] = v.split(":").map(Number); makeTuplet(a, n); } }}>
          <option value="">Tuplet…</option>
          <option value="3:2">Tresillo 3:2 (Ctrl+3)</option>
          <option value="5:4">Quintillo 5:4 (Ctrl+5)</option>
          <option value="6:4">Seisillo 6:4 (Ctrl+6)</option>
          <option value="7:4">Septillo 7:4 (Ctrl+7)</option>
          <option value="7:8">Septillo 7:8</option>
          <option value="2:3">Dosillo 2:3</option>
          <option value="4:3">Cuatrillo 4:3</option>
          <option value="clear">Quitar tuplet (Ctrl+0)</option>
        </select>
        <button onClick={toggleTie} title="Ligadura de prolongación (T)">Ligadura</button>
        <button onClick={() => addSpanner("slur")} title="Ligadura de expresión sobre la selección (S). Mayús+flechas selecciona varias notas">⌒ Expresión</button>
        <button onClick={() => addSpanner("crescendo")} title="Crescendo (<)">&lt; Cresc.</button>
        <button onClick={() => addSpanner("decrescendo")} title="Decrescendo (>)">&gt; Dim.</button>
        <button onClick={removeSpanners} title="Quitar ligaduras de expresión y reguladores de la selección">Quitar expresión</button>
        <select class="assist-select" title="Qué transpone «Transponer…»: toda la pieza, o desde el compás del cursor hasta el final (la armadura vuelve a cambiar en ese compás)" value={transposeScope()} onChange={(e) => setTransposeScope(e.currentTarget.value as "all" | "from")}>
          <option value="all">Toda la pieza</option>
          <option value="from">Desde el compás del cursor</option>
        </select>
        <select
          class="assist-select"
          title="Transpone todas las partes con altura, con su armadura (la más cercana a la nueva tónica) y el cifrado de acordes. Las notas se escriben con la armadura nueva; las cuerdas fijadas en la tablatura se sueltan"
          value=""
          onChange={(e) => { const v = Number(e.currentTarget.value); e.currentTarget.value = ""; if (v) void doTransposeRange(v); }}
        >
          <option value="">Transponer…</option>
          <option value="1">+1 semitono</option>
          <option value="2">+2 (un tono)</option>
          <option value="3">+3 (tercera menor)</option>
          <option value="5">+5 (cuarta)</option>
          <option value="7">+7 (quinta)</option>
          <option value="12">+12 (octava)</option>
          <option value="-1">−1 semitono</option>
          <option value="-2">−2 (un tono)</option>
          <option value="-3">−3 (tercera menor)</option>
          <option value="-5">−5 (cuarta)</option>
          <option value="-7">−7 (quinta)</option>
          <option value="-12">−12 (octava)</option>
        </select>
        <button onClick={() => transpose(1)} title="↑">Semitono ↑</button>
        <button onClick={() => transpose(-1)} title="↓">Semitono ↓</button>
        <button onClick={() => run(api.deleteEvent(cursor().part, cursor().voice, cursor().m, cursor().start))} title="Supr">Borrar</button>
        <button onClick={() => run(api.addMeasures(4))}>+ 4 compases</button>
      </div>

      <div class="toolbar palette">
        <span class="label">Dinámicas</span>
        <For each={[...DYNAMICS]}>
          {(d) => (
            <button class="dyn" classList={{ on: !!score() && voiceEvents(score()!.parts[cursor().part].measures[cursor().m], cursor().voice)?.find((x) => x.start === cursor().start)?.dynamic === d }} title={`Dinámica ${d} en la nota seleccionada`} onClick={() => setDynamic(d)}>
              {d}
            </button>
          )}
        </For>
        <span class="sep" />
        <span class="label">Articulaciones</span>
        <button title="Staccato (Mayús+S)" onClick={() => toggleArtic("staccato")}>•</button>
        <button title="Staccatissimo" onClick={() => toggleArtic("staccatissimo")}>▾</button>
        <button title="Tenuto (Mayús+T)" onClick={() => toggleArtic("tenuto")}>–</button>
        <button title="Acento" onClick={() => toggleArtic("accent")}>&gt;</button>
        <button title="Marcato (Mayús+M)" onClick={() => toggleArtic("marcato")}>^</button>
        <button title="Portato (tenuto con staccato)" onClick={() => toggleArtic("portato")}>–•</button>
        <button title="Mordente: nota, la de abajo, nota" onClick={() => toggleArtic("mordent")}>mord</button>
        <button title="Mordente superior (pralltriller): nota, la de arriba, nota" onClick={() => toggleArtic("shorttrill")}>prall</button>
        <button title="Grupeto: arriba, nota, abajo, nota" onClick={() => toggleArtic("turn")}>grup</button>
        <button title="Grupeto invertido: abajo, nota, arriba, nota" onClick={() => toggleArtic("invertedturn")}>grup↺</button>
        <Show when={fretTuning()}>
          <span class="sep" />
          <span class="label">Tablatura</span>
          <button classList={{ on: fretMode() }} title="Entrada por traste (F2): ↑↓ elige la cuerda, 0–9 escribe el traste, [ ] duración, R silencio" onClick={toggleFretMode}>Traste{fretMode() ? ` · cuerda ${fretString()}` : ""}</button>
          <button classList={{ on: curTech() === "hammer" }} title="Ligado: martillo (H) si la siguiente nota es más aguda, tirón (P) si es más grave" onClick={() => setTech("hammer")}>H/P</button>
          <button classList={{ on: curTech() === "slide" }} title="Deslizamiento hasta la siguiente nota" onClick={() => setTech("slide")}>Desliz.</button>
          <For each={[1, 2, 3, 4]}>
            {(n) => <button classList={{ on: typeof curTech() === "object" && (curTech() as { bend: number }).bend === n }} title={`Bending de ${["", "medio tono", "un tono", "tono y medio", "dos tonos"][n]}`} onClick={() => setTech({ bend: n })}>{["", "½", "1", "1½", "2"][n]}↗</button>}
          </For>
        </Show>
        <span class="sep" />
        <span class="label">Líneas</span>
        <button title="8va: suena una octava más alta que lo escrito" onClick={() => addSpanner("ottava8va")}>8va</button>
        <button title="8vb: suena una octava más baja que lo escrito" onClick={() => addSpanner("ottava8vb")}>8vb</button>
        <button title="15ma: dos octavas más alto" onClick={() => addSpanner("ottava15ma")}>15ma</button>
        <button title="15mb: dos octavas más bajo" onClick={() => addSpanner("ottava15mb")}>15mb</button>
        <button title="Pedal de sostenuto del piano" onClick={() => addSpanner("pedal")}>Ped.</button>
        <button title="Trino sobre la selección" onClick={() => addSpanner("trill")}>tr</button>
        <span class="sep" />
        <span class="label">Repeticiones</span>
        <button title="Signo de repetición al inicio del compás seleccionado" onClick={() => toggleRepeat(true)}>|:</button>
        <button title="Signo de repetición al final del compás seleccionado" onClick={() => toggleRepeat(false)}>:|</button>
        <button title="Primera casilla sobre los compases seleccionados" onClick={() => addVolta(1)}>1.</button>
        <button title="Segunda casilla sobre los compases seleccionados" onClick={() => addVolta(2)}>2.</button>
        <span class="sep" />
        <span class="label">Saltos</span>
        <button title="Segno al inicio del compás seleccionado (destino de D.S.)" onClick={() => toggleMarker("Segno")}>𝄋</button>
        <button title="Coda al inicio del compás seleccionado (destino de «al Coda»)" onClick={() => toggleMarker("Coda")}>𝄌</button>
        <button title="«To Coda» al final del compás seleccionado" onClick={() => toggleMarker("ToCoda")}>To Coda</button>
        <button title="Fine al final del compás seleccionado" onClick={() => toggleMarker("Fine")}>Fine</button>
        <select class="tuplet-select" value="" title="Salto al final del compás seleccionado (elegir el mismo otra vez lo quita)" onChange={(e) => { const v = e.currentTarget.value as Jump | ""; e.currentTarget.value = ""; if (v) setJump(v); }}>
          <option value="">Salto…</option>
          <For each={Object.keys(JUMP_TEXT) as Jump[]}>{(j) => <option value={j}>{JUMP_TEXT[j]}</option>}</For>
        </select>
        <span class="hint">Afectan a las notas seleccionadas (Mayús+flechas)</span>
      </div>

      <div class="workspace">
        <main class="canvas">
          <Show when={score() && layout()} fallback={<p class="loading">Cargando…</p>}>
            <div class="page" style={{ width: `${zoom()}%` }}>
              <ScoreView score={shown()!} layout={layout()!} cursor={cursor()} selection={selection()} playing={playing()} inputMode={inputMode()} onPick={onPick} onChordClick={(c, x, y) => openChordEditor(c.measure, c.tick, x, y)} onTabClick={cycleString} fretLine={fretMode() && fretTuning() ? { part: cursor().part, string: fretString() } : null} />
            </div>
          </Show>
        </main>
        <Show when={score()}>
          <aside class="side">
            <h3>Instrumentos</h3>
            <For each={score()!.parts}>
              {(p, i) => (
                <div class="inst" classList={{ sel: cursor().part === i() }} onClick={() => selectPart(i())}>
                  <div class="inst-head">
                    <input class="inst-name" value={p.name} onChange={(e) => run(api.updatePart(i(), { name: e.currentTarget.value }))} onClick={(e) => e.stopPropagation()} />
                    <button title="Quitar instrumento" disabled={score()!.parts.length < 2} onClick={(e) => { e.stopPropagation(); run(api.removePart(i())); }}>✕</button>
                  </div>
                  <select
                    value={instrumentIndex(p.program, p.clef, p.percussion, p.transpose)}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => { const ins = INSTRUMENTS[Number(e.currentTarget.value)]; if (ins) run(api.updatePart(i(), { program: ins.program, clef: ins.clef, percussion: ins.percussion ?? false, transpose: ins.transpose ?? null, tuning: ins.tuning ?? null })); }}
                  >
                    <Show when={instrumentIndex(p.program, p.clef, p.percussion, p.transpose) < 0}><option value="-1">Programa GM {p.program + 1}</option></Show>
                    <For each={INSTRUMENTS}>{(ins, k) => <option value={k()}>{ins.name}</option>}</For>
                  </select>
                  <select
                    class="tab-select"
                    title="Tablatura: dibuja un pentagrama de tablatura bajo el pentagrama"
                    value={(p.tab_only ? 10 : 0) + TUNINGS.findIndex((t) => t.strings.join() === (p.tuning ?? []).join())}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => { const v = Number(e.currentTarget.value); const only = v >= 10; const k = only ? v - 10 : v; run(api.updatePart(i(), k >= 0 ? { tuning: TUNINGS[k].strings, tab_only: only, ...(TUNINGS[k].octave && !p.transpose ? { transpose: OCTAVE_BELOW } : {}) } : { tuning: null, tab_only: false })); }}
                  >
                    <option value="-1">{p.tuning && TUNINGS.findIndex((t) => t.strings.join() === p.tuning!.join()) < 0 ? "Tablatura (afinación propia)" : "Sin tablatura"}</option>
                    <For each={TUNINGS}>{(t, k) => <option value={k()}>Tablatura: {t.name}</option>}</For>
                    <For each={TUNINGS}>{(t, k) => <option value={10 + k()}>Solo tablatura: {t.name}</option>}</For>
                  </select>
                  <select
                    class="tab-select"
                    title="Tonalidad propia de esta parte (armadura de concierto). «La de la pieza» la hace seguir la tonalidad y los cambios de la partitura. Las notas no se mueven: solo cambia la armadura que se dibuja"
                    value={p.key ? String(p.key.fifths) : "x"}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => { const v = e.currentTarget.value; run(api.updatePart(i(), { key: v === "x" ? null : Number(v) })); }}
                  >
                    <option value="x">Tonalidad: la de la pieza</option>
                    <For each={KEY_NAMES}>{(n, k) => <option value={String(k() - 7)}>Tonalidad propia: {n}</option>}</For>
                  </select>
                  <div class="inst-mix" onClick={(e) => e.stopPropagation()}>
                    <button classList={{ on: p.muted }} title="Silenciar" onClick={() => run(api.updatePart(i(), { muted: !p.muted }))}>M</button>
                    <button classList={{ on: p.solo, solo: p.solo }} title="Solo" onClick={() => run(api.updatePart(i(), { solo: !p.solo }))}>S</button>
                    <input type="range" min="0" max="127" value={p.volume} title={`Volumen ${p.volume}`} onChange={(e) => run(api.updatePart(i(), { volume: Number(e.currentTarget.value) }))} />
                  </div>
                </div>
              )}
            </For>
            <div class="inst-add">
              <select value={newInstrument()} onChange={(e) => setNewInstrument(Number(e.currentTarget.value))}>
                <For each={INSTRUMENTS}>{(ins, k) => <option value={k()}>{ins.name}</option>}</For>
              </select>
              <button class="primary" onClick={addInstrument}>+ Añadir</button>
            </div>
          </aside>
        </Show>
      </div>

      <footer class="status">
        <span class="mode" classList={{ on: inputMode() }}>{inputMode() ? "ENTRADA" : "SELECCIÓN"}</span>
        <span>Compás {cursor().m + 1} · Voz {cursor().voice + 1}</span>
        <span>Duración: {ticks()} ticks</span>
        <span class="grow">{status()}</span>
      </footer>

      <Show when={textEdit()}>
        {(ed) => (
          <div class="text-editor" style={{ position: "fixed", left: "50%", transform: "translateX(-50%)", bottom: "48px", "z-index": 50, background: "var(--panel, #fff)", border: "1px solid var(--ink-dim, #888)", "border-radius": "6px", padding: "8px", display: "flex", gap: "6px", "align-items": "center", "box-shadow": "0 2px 10px rgba(0,0,0,.3)" }}>
            <span>Texto sobre el pentagrama:</span>
            <input
              ref={(el) => setTimeout(() => { el.focus(); el.select(); })}
              value={ed().text}
              size={20}
              placeholder="dolce, pizz., rit.…"
              onKeyDown={(e) => {
                if (e.key === "Enter") { e.preventDefault(); void commitText(e.currentTarget.value); }
                else if (e.key === "Escape") { e.preventDefault(); setTextEdit(null); }
              }}
            />
            <button onClick={() => void commitText("")} title="Borrar el texto">Borrar</button>
            <button onClick={() => setTextEdit(null)}>Cancelar</button>
          </div>
        )}
      </Show>

      <Show when={lyricEdit()}>
        {(ed) => (
          <div class="lyric-editor" style={{ position: "fixed", left: "50%", transform: "translateX(-50%)", bottom: "48px", "z-index": 50, background: "var(--panel, #fff)", border: "1px solid var(--ink-dim, #888)", "border-radius": "6px", padding: "8px", "box-shadow": "0 2px 10px rgba(0,0,0,.3)", display: "flex", gap: "6px", "align-items": "center" }}>
            <span>Letra, estrofa</span>
            <select value={ed().verse} onChange={(e) => startLyrics(Number(e.currentTarget.value))}>
              {[0, 1, 2, 3].map((v) => <option value={v}>{v + 1}</option>)}
            </select>
            <input
              ref={(el) => setTimeout(() => { el.focus(); el.select(); })}
              value={ed().text}
              size={14}
              placeholder="sílaba"
              onInput={(e) => setLyricEdit({ ...ed(), text: e.currentTarget.value })}
              onKeyDown={(e) => {
                if (e.key === " " || e.key === "Enter") { e.preventDefault(); void lyricCommit(e.currentTarget.value, false); }
                else if (e.key === "-") { e.preventDefault(); void lyricCommit(e.currentTarget.value, true); }
                else if (e.key === "_") { e.preventDefault(); void lyricUnderscore(e.currentTarget.value); }
                else if (e.key === "Delete" && e.currentTarget.value === "") { e.preventDefault(); void lyricClearExtend(); }
                else if (e.key === "Backspace" && e.currentTarget.value === "") { e.preventDefault(); lyricBack(); }
                else if (e.key === "Escape") { e.preventDefault(); setLyricEdit(null); }
              }}
            />
            <button onClick={() => void lyricClearExtend()} title="Quita la línea de melisma de esta nota (también Supr con el campo vacío)">Quitar línea</button>
            <button onClick={() => setLyricEdit(null)}>Terminar</button>
          </div>
        )}
      </Show>

      <Show when={chordEdit()}>
        {(ed) => (
          <div class="chord-editor" style={{ position: "fixed", left: `${Math.max(8, Math.min(ed().x - 20, window.innerWidth - 300))}px`, top: `${Math.max(8, ed().y + 14)}px`, "z-index": 50, background: "var(--panel, #fff)", border: "1px solid var(--ink-dim, #888)", "border-radius": "6px", padding: "8px", "box-shadow": "0 2px 10px rgba(0,0,0,.3)", display: "flex", gap: "6px", "align-items": "center" }}>
            <span>Compás {ed().measure + 1}, tiempo {ed().tick / 480 + 1}:</span>
            <input
              ref={(el) => setTimeout(() => { el.focus(); el.select(); })}
              value={ed().text}
              size={9}
              placeholder="Am7, F/A…"
              onKeyDown={(e) => {
                if (e.key === "Enter") { e.preventDefault(); void commitChord(e.currentTarget.value); }
                else if (e.key === "Escape") { e.preventDefault(); setChordEdit(null); }
              }}
            />
            <button onClick={() => void commitChord("")} title="Borrar este acorde">Borrar</button>
            <button onClick={() => setChordEdit(null)}>Cancelar</button>
          </div>
        )}
      </Show>

      <Show when={showNew()}>
        <NewDialog
          onCancel={() => setShowNew(false)}
          onCreate={async (t, n, d, f, m) => {
            setShowNew(false);
            setCursor({ part: 0, voice: 0, m: 0, start: 0 });
            if (await run(api.newScore(t, n, d, f, m), false)) {
              setDirty(false);
              setLastMidi(67);
            }
          }}
        />
      </Show>

      <Show when={issues()}>
        {(list) => (
          <div class="modal" onClick={() => setIssues(null)}>
            <div class="dialog" onClick={(e) => e.stopPropagation()}>
              <h2>Revisión de la partitura</h2>
              <p>{list().length} aviso(s). Son sugerencias, no errores seguros: la música puede romper estas reglas a propósito. Pulsa uno para ir al compás.</p>
              <ul class="issues" style={{ "max-height": "50vh", "overflow-y": "auto", "padding-left": "18px", margin: 0 }}>
                <For each={list()}>
                  {(it) => (
                    <li style={{ cursor: "pointer", margin: "4px 0" }} onClick={() => { setCursor({ part: it.part, voice: it.voice, m: it.measure, start: it.tick }); setIssues(null); setStatus(it.message); }}>
                      <b>Compás {it.measure + 1}</b>, {score()!.parts[it.part].name}: {it.message}
                    </li>
                  )}
                </For>
              </ul>
              <div class="actions">
                <button type="button" class="primary" onClick={() => setIssues(null)}>Cerrar</button>
              </div>
            </div>
          </div>
        )}
      </Show>

      <Show when={omrSetup()}>
        <div class="modal" onClick={() => setOmrSetup(false)}>
          <div class="dialog" onClick={(e) => e.stopPropagation()}>
            <h2>Importar partitura impresa</h2>
            <p>
              NovaScore reconoce partituras escaneadas o en PDF con <b>Audiveris</b>, un programa libre (AGPL-3) que se instala aparte y trabaja en tu equipo, sin enviar nada a internet.
            </p>
            <ol>
              <li>Descarga Audiveris para tu sistema en <b>github.com/Audiveris/audiveris/releases</b> (el instalador de Windows incluye Java; en Linux y macOS hace falta Java 17 o superior).</li>
              <li>Instálalo y vuelve a pulsar «Importar partitura impresa»: se detecta solo en las carpetas habituales.</li>
              <li>Si lo instalaste en otro sitio, elige su ejecutable.</li>
            </ol>
            <p>El resultado es un borrador: revisa las notas, los ritmos y las alteraciones frente a la partitura original.</p>
            <div class="actions">
              <button type="button" onClick={() => setOmrSetup(false)}>Cerrar</button>
              <button type="button" class="primary" onClick={chooseAudiveris}>Elegir ejecutable de Audiveris…</button>
            </div>
          </div>
        </div>
      </Show>

      <Show when={omrBusy()}>
        {(name) => (
          <div class="modal">
            <div class="dialog">
              <h2>Reconociendo partitura…</h2>
              <p title={name()}>{name()}</p>
              <p>Audiveris está leyendo la partitura. Puede tardar varios minutos según el número de páginas.</p>
              <div class="actions">
                <button type="button" onClick={() => api.cancelOmr()}>Cancelar</button>
              </div>
            </div>
          </div>
        )}
      </Show>

      <Show when={audioFile()}>
        {(path) => <AudioDialog path={path()} onCancel={() => setAudioFile(null)} onImport={doImportAudio} />}
      </Show>
    </div>
  );
}

function AudioDialog(props: { path: string; onCancel: () => void; onImport: (path: string, o: AudioOptions) => void }) {
  const [auto, setAuto] = createSignal(true);
  const [tempo, setTempo] = createSignal(100);
  const [time, setTime] = createSignal("auto");
  const [amp, setAmp] = createSignal(0.5);
  const [voices, setVoices] = createSignal(1);
  const [chords, setChords] = createSignal(false);
  const [engine, setEngine] = createSignal<"general" | "piano">("general");
  const [pianoModel, setPianoModel] = createSignal<string | null>(null);
  api.pianoModelStatus().then(setPianoModel).catch(() => setPianoModel(null));
  const [dl, setDl] = createSignal<{ done: number; total: number } | null>(null);
  const [dlError, setDlError] = createSignal("");
  const downloadModel = async () => {
    setDlError("");
    setDl({ done: 0, total: 98684797 });
    const un = await listen<{ done: number; total: number }>("piano-download", (e) => setDl(e.payload));
    try {
      setPianoModel(await api.downloadPianoModel());
      setEngine("piano");
    } catch (e) {
      // una cancelación del usuario no es un error que mostrar
      if (!String(e).includes("cancelada")) setDlError(String(e));
    } finally {
      un();
      setDl(null);
    }
  };
  const [confirmDel, setConfirmDel] = createSignal(false);
  const deleteModel = async () => {
    try {
      await api.deletePianoModel();
      setPianoModel(null);
      if (engine() === "piano") setEngine("general");
    } catch (e) {
      setDlError(String(e));
    }
    setConfirmDel(false);
  };
  const name = () => props.path.split(/[\\/]/).pop() ?? props.path;
  return (
    <div class="modal" onClick={props.onCancel}>
      <form
        class="dialog"
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          const [num, den] = time() === "auto" ? [undefined, undefined] : time().split("/").map(Number);
          props.onImport(props.path, { engine: engine(), minRelative: engine() === "piano" ? undefined : amp(), tempo: auto() ? undefined : tempo(), num, den, maxVoices: voices(), chords: chords() });
        }}
      >
        <h2>Importar audio</h2>
        <p title={props.path}>{name()}</p>
        <label>Motor
          <select value={engine()} onChange={(e) => setEngine(e.currentTarget.value as "general" | "piano")}>
            <option value="general">General (cualquier instrumento)</option>
            <option value="piano" disabled={!pianoModel()}>{pianoModel() ? "Piano (alta precisión, usa la GPU)" : "Piano (alta precisión): modelo no instalado"}</option>
          </select>
        </label>
        <Show when={!pianoModel()}>
          <p>
            El motor de piano usa un modelo de 98 MB (licencia CC-BY-4.0, Kong et al.) que se descarga una sola vez y se guarda en tu equipo.{" "}
            <button type="button" disabled={dl() !== null} onClick={downloadModel}>
              {dl() ? `Descargando… ${Math.round((dl()!.done / dl()!.total) * 100)} %` : "Descargar modelo de piano"}
            </button>
            <Show when={dl() !== null}>
              {" "}<button type="button" onClick={() => api.cancelPianoDownload()}>Cancelar descarga</button>
            </Show>
          </p>
          <Show when={dlError()}><p class="error">{dlError()}</p></Show>
        </Show>
        <Show when={pianoModel()}>
          <p>
            Modelo de piano instalado (98 MB).{" "}
            <Show when={!confirmDel()} fallback={
              <>
                ¿Borrarlo? Se puede volver a descargar.{" "}
                <button type="button" onClick={deleteModel}>Sí, borrar</button>{" "}
                <button type="button" onClick={() => setConfirmDel(false)}>No</button>
              </>
            }>
              <button type="button" onClick={() => setConfirmDel(true)}>Borrar modelo de piano</button>
            </Show>
          </p>
          <Show when={dlError()}><p class="error">{dlError()}</p></Show>
        </Show>
        <label>Tempo
          <select value={auto() ? "auto" : "fixed"} onChange={(e) => setAuto(e.currentTarget.value === "auto")}>
            <option value="auto">Automático</option>
            <option value="fixed">Indicar…</option>
          </select>
        </label>
        <Show when={!auto()}>
          <label>{["6/8", "9/8", "12/8"].includes(time()) ? "Pulsos (negra con puntillo) por minuto" : "Negras por minuto"}
            <input type="number" min="20" max="300" value={tempo()} onInput={(e) => setTempo(Number(e.currentTarget.value) || 100)} />
          </label>
        </Show>
        <label>Compás
          <select value={time()} onChange={(e) => setTime(e.currentTarget.value)}>
            <option value="auto">Automático (2/4, 3/4, 4/4 y anacrusa)</option>
            <For each={["2/4", "3/4", "4/4", "5/4", "6/8", "9/8", "12/8", "2/2", "3/8"]}>{(t) => <option>{t}</option>}</For>
          </select>
        </label>
        <label>Voces por pentagrama
          <select value={voices()} onChange={(e) => setVoices(Number(e.currentTarget.value))}>
            <option value="1">1 (más limpio)</option>
            <option value="2">2 (separa notas largas)</option>
          </select>
        </label>
        <label>
          <input type="checkbox" checked={chords()} onChange={(e) => setChords(e.currentTarget.checked)} /> Detectar también los acordes (cifrado sobre el pentagrama; borrador)
        </label>
        <Show when={engine() === "general"}>
        <label>Sensibilidad: se descartan las notas que suenan menos de {Math.round(amp() * 100)} % de lo que suenan las fuertes
          <input type="range" min="0.4" max="0.9" step="0.05" value={amp()} onInput={(e) => setAmp(Number(e.currentTarget.value))} />
        </label>
        <p>Un porcentaje menor = más notas, pero con más falsas (armónicos). Es un borrador: revísalo.</p>
        </Show>
        <div class="actions">
          <button type="button" onClick={props.onCancel}>Cancelar</button>
          <button type="submit" class="primary">Transcribir</button>
        </div>
      </form>
    </div>
  );
}

function NewDialog(props: {
  onCancel: () => void;
  onCreate: (title: string, num: number, den: number, fifths: number, measures: number) => void;
}) {
  const [title, setTitle] = createSignal("Sin título");
  const [time, setTime] = createSignal("4/4");
  const [fifths, setFifths] = createSignal(0);
  const [measures, setMeasures] = createSignal(16);
  return (
    <div class="modal" onClick={props.onCancel}>
      <form
        class="dialog"
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          const [n, d] = time().split("/").map(Number);
          props.onCreate(title(), n, d, fifths(), measures());
        }}
      >
        <h2>Nueva partitura</h2>
        <label>Título<input value={title()} onInput={(e) => setTitle(e.currentTarget.value)} autofocus /></label>
        <label>Compás
          <select value={time()} onChange={(e) => setTime(e.currentTarget.value)}>
            <For each={["2/4", "3/4", "4/4", "5/4", "6/8", "9/8", "12/8", "2/2", "3/8"]}>{(t) => <option>{t}</option>}</For>
          </select>
        </label>
        <label>Tonalidad
          <select value={fifths()} onChange={(e) => setFifths(Number(e.currentTarget.value))}>
            <For each={KEY_NAMES}>{(n, i) => <option value={i() - 7}>{n}</option>}</For>
          </select>
        </label>
        <label>Compases
          <input type="number" min="1" max="500" value={measures()} onInput={(e) => setMeasures(Number(e.currentTarget.value) || 1)} />
        </label>
        <div class="actions">
          <button type="button" onClick={props.onCancel}>Cancelar</button>
          <button type="submit" class="primary">Crear</button>
        </div>
      </form>
    </div>
  );
}
