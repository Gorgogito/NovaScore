import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { api, AudioOptions } from "./api";
import { drumFromPos, DRUM_KEYS } from "./drum";
import { INSTRUMENTS } from "./instruments";
import { keyAlter, layoutScore } from "./layout";
import { scoreToSvgPages } from "./pdfExport";
import { Player } from "./playback";
import { ScoreView } from "./ScoreView";
import { ArticKind, Cursor, displayScore, DYNAMICS, measureLen, ottavaShiftAt, Pitch, pitchMidi, Score, Selection, SpannerKind, timeAt, voiceEvents } from "./types";
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
  const [lastMidi, setLastMidi] = createSignal(67);
  const [playing, setPlaying] = createSignal<string | null>(null);
  const [isPlaying, setIsPlaying] = createSignal(false);
  const [zoom, setZoom] = createSignal(100);
  const [status, setStatus] = createSignal("Pulsa N para entrar en modo de entrada de notas.");
  const [showNew, setShowNew] = createSignal(false);
  const [audioFile, setAudioFile] = createSignal<string | null>(null);
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
  const pitchFor = (step: number, alterOverride: number | null): Pitch => {
    const alter = alterOverride ?? keyAlter(score()!.fifths, step);
    let best: Pitch = { step, alter, octave: 4 };
    let bestD = Infinity;
    for (let o = 0; o <= 8; o++) {
      const p = { step, alter, octave: o };
      const d = Math.abs(pitchMidi(p) - lastMidi());
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
  const removeSpanners = async () => {
    const r = currentRange();
    const toEv = voiceEvents(score()!.parts[r.part].measures[r.to.m], r.voice)?.find((e) => e.start === r.to.start);
    const end = { measure: r.to.m, tick: (toEv?.start ?? r.to.start) + (toEv?.ticks ?? 0) };
    await run(api.removeSpanners(r.part, r.voice, { measure: r.from.m, tick: r.from.start }, end));
  };

  const enter = async (p: Pitch) => {
    const c = cursor();
    try {
      const r = await api.enterNote(c.part, c.voice, c.m, c.start, ticks(), p);
      setScore(r.score);
      setDirty(true);
      setLastEntered(c);
      setCursor({ ...c, m: r.measure, start: r.start });
      setLastMidi(pitchMidi(p));
      setPendingAlter(null);
    } catch (e) {
      setStatus(String(e));
    }
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
    const alter = pendingAlter() ?? keyAlter(score()!.fifths, step);
    let p: Pitch | null = null;
    for (let o = 0; o <= 8 && !p; o++) {
      const cand = { step, alter, octave: o };
      if (pitchMidi(cand) > top) p = cand;
    }
    if (!p) return;
    if (await run(api.addPitch(t.part, t.voice, t.m, t.start, p))) {
      setPendingAlter(null);
      setLastMidi(pitchMidi(p));
    }
  };

  const [newInstrument, setNewInstrument] = createSignal(6);
  const instrumentIndex = (program: number, clef: string, percussion: boolean) => {
    if (percussion) return INSTRUMENTS.findIndex((i) => i.percussion);
    const exact = INSTRUMENTS.findIndex((i) => !i.percussion && i.program === program && i.clef === clef);
    return exact >= 0 ? exact : INSTRUMENTS.findIndex((i) => !i.percussion && i.program === program);
  };
  const selectPart = (part: number) => {
    const c = cursor();
    const evs = voiceEvents(score()!.parts[part].measures[c.m], c.voice);
    const ev = evs ? (evs.find((e) => e.start <= c.start && c.start < e.start + e.ticks) ?? evs[0]) : null;
    setCursor({ ...c, part, start: ev ? ev.start : 0 });
  };
  const addInstrument = async () => {
    const ins = INSTRUMENTS[newInstrument()];
    if (await run(api.addPart(ins.name, ins.clef, ins.program, ins.percussion ?? false))) selectPart(score()!.parts.length - 1);
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
      const p: Pitch = { step, alter: pendingAlter() ?? keyAlter(s.fifths, step), octave: Math.floor(idx / 7) + ottavaShiftAt(s.parts[part], m, start) };
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
      xml: [{ name: "MusicXML", extensions: ["musicxml", "xml"] }],
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
    if (!score() || showNew() || audioFile()) return;
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
      else if (k === "0") await clearTuplet();
      else if (e.key === "ArrowUp") await transpose(12);
      else if (e.key === "ArrowDown") await transpose(-12);
      else return;
      e.preventDefault();
      return;
    }
    if (e.altKey) return;
    e.preventDefault();
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
    else if (k === "delete") await run(api.deleteEvent(cursor().part, cursor().voice, cursor().m, cursor().start));
    else if (k === "backspace") {
      await run(api.deleteEvent(cursor().part, cursor().voice, cursor().m, cursor().start));
      move(-1);
    } else if (k === "t") await toggleTie();
    else if (k in LETTER_STEP && e.shiftKey) await addToChord(LETTER_STEP[k]);
    else if (inputMode() && score()!.parts[cursor().part].percussion && k in DRUM_KEYS) await enter(pitchFromMidi(DRUM_KEYS[k]));
    else if (inputMode() && k in LETTER_STEP) await enter(pitchFor(LETTER_STEP[k], pendingAlter()));
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
        <button onClick={pickAudio} title="Transcribir un archivo de audio (wav, mp3, flac, ogg, m4a) a partitura">Importar audio</button>
        <button onClick={doExportMuseScore} title="Guardar como MuseScore 4 (.mscz)">Exportar MuseScore</button>
        <button onClick={() => doExport("xml")} title="Exportar MusicXML">Exportar XML</button>
        <button onClick={() => doExport("midi")} title="Exportar MIDI">Exportar MIDI</button>
        <button onClick={doExportPdf} title="Exportar PDF">Exportar PDF</button>
        <span class="sep" />
        <button onClick={() => run(api.undo())} title="Deshacer (Ctrl+Z)">↶</button>
        <button onClick={() => run(api.redo())} title="Rehacer (Ctrl+Y)">↷</button>
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
            <select value={score()!.fifths} onChange={(e) => run(api.setFifths(Number(e.currentTarget.value)))}>
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
          <option value="2:3">Dosillo 2:3</option>
          <option value="4:3">Cuatrillo 4:3</option>
          <option value="clear">Quitar tuplet (Ctrl+0)</option>
        </select>
        <button onClick={toggleTie} title="Ligadura de prolongación (T)">Ligadura</button>
        <button onClick={() => addSpanner("slur")} title="Ligadura de expresión sobre la selección (S). Mayús+flechas selecciona varias notas">⌒ Expresión</button>
        <button onClick={() => addSpanner("crescendo")} title="Crescendo (<)">&lt; Cresc.</button>
        <button onClick={() => addSpanner("decrescendo")} title="Decrescendo (>)">&gt; Dim.</button>
        <button onClick={removeSpanners} title="Quitar ligaduras de expresión y reguladores de la selección">Quitar expresión</button>
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
        <span class="hint">Afectan a las notas seleccionadas (Mayús+flechas)</span>
      </div>

      <div class="workspace">
        <main class="canvas">
          <Show when={score() && layout()} fallback={<p class="loading">Cargando…</p>}>
            <div class="page" style={{ width: `${zoom()}%` }}>
              <ScoreView score={shown()!} layout={layout()!} cursor={cursor()} selection={selection()} playing={playing()} inputMode={inputMode()} onPick={onPick} />
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
                    value={instrumentIndex(p.program, p.clef, p.percussion)}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => { const ins = INSTRUMENTS[Number(e.currentTarget.value)]; if (ins) run(api.updatePart(i(), { program: ins.program, clef: ins.clef, percussion: ins.percussion ?? false })); }}
                  >
                    <Show when={instrumentIndex(p.program, p.clef, p.percussion) < 0}><option value="-1">Programa GM {p.program + 1}</option></Show>
                    <For each={INSTRUMENTS}>{(ins, k) => <option value={k()}>{ins.name}</option>}</For>
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
          props.onImport(props.path, { engine: engine(), minRelative: engine() === "piano" ? undefined : amp(), tempo: auto() ? undefined : tempo(), num, den, maxVoices: voices() });
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
