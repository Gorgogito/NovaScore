/** General MIDI instruments offered in the UI (program number is 0-based). */
export interface Instrument {
  name: string;
  program: number;
  clef: "treble" | "bass";
  /** Drum staff (MIDI channel 10). */
  percussion?: boolean;
}

export const INSTRUMENTS: Instrument[] = [
  { name: "Piano", program: 0, clef: "treble" },
  { name: "Piano (mano izquierda)", program: 0, clef: "bass" },
  { name: "Órgano", program: 19, clef: "treble" },
  { name: "Guitarra de nailon", program: 24, clef: "treble" },
  { name: "Guitarra de acero", program: 25, clef: "treble" },
  { name: "Bajo eléctrico", program: 33, clef: "bass" },
  { name: "Violín", program: 40, clef: "treble" },
  { name: "Viola", program: 41, clef: "treble" },
  { name: "Violonchelo", program: 42, clef: "bass" },
  { name: "Contrabajo", program: 43, clef: "bass" },
  { name: "Arpa", program: 46, clef: "treble" },
  { name: "Cuerdas (conjunto)", program: 48, clef: "treble" },
  { name: "Coro", program: 52, clef: "treble" },
  { name: "Trompeta", program: 56, clef: "treble" },
  { name: "Trombón", program: 57, clef: "bass" },
  { name: "Tuba", program: 58, clef: "bass" },
  { name: "Trompa", program: 60, clef: "treble" },
  { name: "Saxo alto", program: 65, clef: "treble" },
  { name: "Saxo tenor", program: 66, clef: "treble" },
  { name: "Oboe", program: 68, clef: "treble" },
  { name: "Fagot", program: 70, clef: "bass" },
  { name: "Clarinete", program: 71, clef: "treble" },
  { name: "Flauta", program: 73, clef: "treble" },
  { name: "Flauta dulce", program: 74, clef: "treble" },
  { name: "Marimba", program: 12, clef: "treble" },
  { name: "Batería / percusión", program: 0, clef: "treble", percussion: true },
];
