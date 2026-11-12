import { invoke } from "@tauri-apps/api/core";
import type { Jump, Marker, Pitch, Score, SpannerKind, SpPos, Transpose } from "./types";

export interface Entered {
  score: Score;
  measure: number;
  start: number;
}
export interface Sound {
  part: number;
  channel: number;
  start: number;
  ticks: number;
  midi: number;
  velocity: number;
}
export interface PartUpdate {
  name?: string;
  clef?: "treble" | "bass";
  program?: number;
  volume?: number;
  muted?: boolean;
  solo?: boolean;
  percussion?: boolean;
  /** `null` clears the transposition. */
  transpose?: Transpose | null;
  /** Tablature: open strings; `null` hides the tab staff. */
  tuning?: number[] | null;
}
export interface PedalEvent {
  part: number;
  channel: number;
  at: number;
  down: boolean;
}
export interface Playback {
  sounds: Sound[];
  channels: number[];
  pedals: PedalEvent[];
  /** Measures in playing order (repeats unfolded). */
  order: number[];
}
type Imported = { score: Score; warnings: string[] };
/** Opciones de la transcripción de audio; sin un campo se usa el valor automático. */
export type AudioOptions = { minAmplitude?: number; minRelative?: number; tempo?: number; num?: number; den?: number; maxVoices?: number; engine?: "general" | "piano"; chords?: boolean };

export const api = {
  getScore: () => invoke<Score>("get_score"),
  newScore: (title: string, num: number, den: number, fifths: number, measures: number) =>
    invoke<Score>("new_score", { title, num, den, fifths, measures }),

  enterNote: (part: number, voice: number, measure: number, start: number, ticks: number, p: Pitch) =>
    invoke<Entered>("enter_note", { part, voice, measure, start, ticks, step: p.step, alter: p.alter, octave: p.octave }),
  addPitch: (part: number, voice: number, measure: number, start: number, p: Pitch) =>
    invoke<Score>("add_pitch", { part, voice, measure, start, step: p.step, alter: p.alter, octave: p.octave }),
  toggleTie: (part: number, voice: number, measure: number, start: number) => invoke<Score>("toggle_tie", { part, voice, measure, start }),
  enterRest: (part: number, voice: number, measure: number, start: number, ticks: number) =>
    invoke<Entered>("enter_rest", { part, voice, measure, start, ticks }),
  makeTuplet: (part: number, voice: number, measure: number, start: number, actual: number, normal: number) =>
    invoke<Score>("make_tuplet", { part, voice, measure, start, actual, normal }),
  clearTuplet: (part: number, voice: number, measure: number, start: number) =>
    invoke<Score>("clear_tuplet", { part, voice, measure, start }),
  deleteEvent: (part: number, voice: number, measure: number, start: number) => invoke<Score>("delete_event", { part, voice, measure, start }),
  transpose: (part: number, voice: number, measure: number, start: number, semitones: number) =>
    invoke<Score>("transpose_event", { part, voice, measure, start, semitones }),

  setText: (part: number, voice: number, measure: number, start: number, text: string) =>
    invoke<Score>("set_text", { part, voice, measure, start, text }),
  setString: (part: number, voice: number, measure: number, start: number, midi: number, string: number) =>
    invoke<Score>("set_string", { part, voice, measure, start, midi, string }),
  setLyricExtend: (part: number, voice: number, measure: number, start: number, verse: number, endMeasure: number | null, endTick: number) =>
    invoke<Score>("set_lyric_extend", { part, voice, measure, start, verse, endMeasure, endTick }),
  setLyric: (part: number, voice: number, measure: number, start: number, verse: number, text: string, syllabic: string) =>
    invoke<Score>("set_lyric", { part, voice, measure, start, verse, text, syllabic }),
  setDynamic: (part: number, voice: number, measure: number, start: number, dynamic: string | null) =>
    invoke<Score>("set_dynamic", { part, voice, measure, start, dynamic }),
  toggleArticulation: (part: number, voice: number, start: SpPos, end: SpPos, kind: string) =>
    invoke<Score>("toggle_articulation", { part, voice, startMeasure: start.measure, startTick: start.tick, endMeasure: end.measure, endTick: end.tick, kind }),
  toggleSpanner: (part: number, kind: SpannerKind, voice: number, start: SpPos, end: SpPos, endings?: number) =>
    invoke<Score>("toggle_spanner", { part, kind, voice, startMeasure: start.measure, startTick: start.tick, endMeasure: end.measure, endTick: end.tick, endings: endings ?? null }),
  toggleRepeat: (measure: number, start: boolean) => invoke<Score>("toggle_repeat", { measure, start }),
  toggleMarker: (measure: number, marker: Marker) => invoke<Score>("toggle_marker", { measure, marker }),
  setJump: (measure: number, jump: Jump | null) => invoke<Score>("set_jump", { measure, jump }),
  removeSpanners: (part: number, voice: number, start: SpPos, end: SpPos) =>
    invoke<Score>("remove_spanners", { part, voice, startMeasure: start.measure, startTick: start.tick, endMeasure: end.measure, endTick: end.tick }),

  addPart: (name: string, clef: "treble" | "bass", program: number, percussion: boolean, transpose?: Transpose, tuning?: number[]) =>
    invoke<Score>("add_part", { name, clef, program, percussion, transpose: transpose ?? null, tuning: tuning ?? null }),
  removePart: (part: number) => invoke<Score>("remove_part", { part }),
  updatePart: (part: number, update: PartUpdate) => invoke<Score>("update_part", { part, update }),

  addMeasures: (count: number) => invoke<Score>("add_measures", { count }),
  setTime: (measure: number, num: number, den: number) => invoke<Score>("set_time", { measure, num, den }),
  setFifths: (fifths: number) => invoke<Score>("set_fifths", { fifths }),
  setTempo: (tempo: number) => invoke<Score>("set_tempo", { tempo }),
  detectChords: () => invoke<Score>("detect_chords"),
  setChord: (measure: number, tick: number, text: string) => invoke<Score>("set_chord", { measure, tick, text }),
  clearChords: () => invoke<Score>("clear_chords"),
  undo: () => invoke<Score>("undo"),
  redo: () => invoke<Score>("redo"),

  getSounds: () => invoke<Playback>("get_sounds"),
  save: (path: string) => invoke<void>("save_score", { path }),
  load: (path: string) => invoke<Score>("load_score", { path }),
  exportPdf: (path: string, svgs: string[]) => invoke<void>("export_pdf", { path, svgs }),
  startupExport: () => invoke<string | null>("startup_export"),
  quit: () => invoke<void>("quit"),
  exportMscz: (path: string) => invoke<void>("export_mscz", { path }),
  exportMidi: (path: string) => invoke<void>("export_midi", { path }),
  exportMusicxml: (path: string) => invoke<void>("export_musicxml", { path }),
  importMusicxml: (path: string) => invoke<Imported>("import_musicxml", { path }),
  importMscz: (path: string) => invoke<Imported>("import_mscz", { path }),
  importMidi: (path: string) => invoke<Imported>("import_midi", { path }),
  pianoModelStatus: () => invoke<string | null>("piano_model_status"),
  downloadPianoModel: () => invoke<string>("download_piano_model"),
  cancelPianoDownload: () => invoke<void>("cancel_piano_download"),
  deletePianoModel: () => invoke<void>("delete_piano_model"),
  omrStatus: () => invoke<{ found: boolean; path?: string; source?: string }>("omr_status"),
  setOmrPath: (path: string | null) => invoke<{ found: boolean; path?: string; source?: string }>("set_omr_path", { path }),
  importOmr: (path: string) => invoke<Imported>("import_omr", { path }),
  cancelOmr: () => invoke<void>("cancel_omr"),
  importAudio: (path: string, options: AudioOptions) => invoke<Imported>("import_audio", { path, options }),
};
