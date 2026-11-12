/** General MIDI instruments offered in the UI (program number is 0-based). */
export interface Instrument {
  name: string;
  program: number;
  clef: "treble" | "bass";
  /** Drum staff (MIDI channel 10). */
  percussion?: boolean;
  /** Transposing instrument: sounding = written + interval (MuseScore's sign). */
  transpose?: { diatonic: number; chromatic: number };
  /** Open strings: the part is also shown as a tablature. */
  tuning?: number[];
}

export const INSTRUMENTS: Instrument[] = [
  { name: "Piano", program: 0, clef: "treble" },
  { name: "Piano (mano izquierda)", program: 0, clef: "bass" },
  { name: "Órgano", program: 19, clef: "treble" },
  { name: "Guitarra de nailon", program: 24, clef: "treble", transpose: { diatonic: -7, chromatic: -12 }, tuning: [40, 45, 50, 55, 59, 64] },
  { name: "Guitarra de acero", program: 25, clef: "treble", transpose: { diatonic: -7, chromatic: -12 }, tuning: [40, 45, 50, 55, 59, 64] },
  { name: "Bajo eléctrico", program: 33, clef: "bass", transpose: { diatonic: -7, chromatic: -12 }, tuning: [28, 33, 38, 43] },
  { name: "Violín", program: 40, clef: "treble" },
  { name: "Viola", program: 41, clef: "treble" },
  { name: "Violonchelo", program: 42, clef: "bass" },
  { name: "Contrabajo", program: 43, clef: "bass", transpose: { diatonic: -7, chromatic: -12 } },
  { name: "Arpa", program: 46, clef: "treble" },
  { name: "Cuerdas (conjunto)", program: 48, clef: "treble" },
  { name: "Coro", program: 52, clef: "treble" },
  { name: "Trompeta en Si♭", program: 56, clef: "treble", transpose: { diatonic: -1, chromatic: -2 } },
  { name: "Trombón", program: 57, clef: "bass" },
  { name: "Tuba", program: 58, clef: "bass" },
  { name: "Trompa en Fa", program: 60, clef: "treble", transpose: { diatonic: -4, chromatic: -7 } },
  { name: "Saxo soprano", program: 64, clef: "treble", transpose: { diatonic: -1, chromatic: -2 } },
  { name: "Saxo alto", program: 65, clef: "treble", transpose: { diatonic: -5, chromatic: -9 } },
  { name: "Saxo tenor", program: 66, clef: "treble", transpose: { diatonic: -8, chromatic: -14 } },
  { name: "Saxo barítono", program: 67, clef: "treble", transpose: { diatonic: -12, chromatic: -21 } },
  { name: "Oboe", program: 68, clef: "treble" },
  { name: "Fagot", program: 70, clef: "bass" },
  { name: "Corno inglés", program: 69, clef: "treble", transpose: { diatonic: -4, chromatic: -7 } },
  { name: "Clarinete en Si♭", program: 71, clef: "treble", transpose: { diatonic: -1, chromatic: -2 } },
  { name: "Clarinete en La", program: 71, clef: "treble", transpose: { diatonic: -2, chromatic: -3 } },
  { name: "Flautín", program: 72, clef: "treble", transpose: { diatonic: 7, chromatic: 12 } },
  { name: "Flauta", program: 73, clef: "treble" },
  { name: "Flauta dulce", program: 74, clef: "treble" },
  { name: "Marimba", program: 12, clef: "treble" },
  { name: "Batería / percusión", program: 0, clef: "treble", percussion: true },
];
