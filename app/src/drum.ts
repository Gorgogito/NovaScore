/** Staff positions (in half spaces above the bottom line) for General MIDI percussion notes. */
const POS: Record<number, number> = {
  35: 1, 36: 1, // bass drum
  37: 5, 38: 5, 39: 5, 40: 5, // side stick, snare, clap
  41: 2, 43: 3, 45: 4, 47: 6, 48: 7, 50: 7, // toms (low -> high)
  42: 9, 44: -1, 46: 9, // closed / pedal / open hi-hat
  49: 10, 57: 10, 52: 10, 55: 10, // crashes
  51: 8, 59: 8, 53: 8, // ride
};

export const drumPos = (midi: number) => POS[midi] ?? 5;

/** Canonical drum note for a clicked staff position (used for mouse entry). */
export const drumFromPos = (pos: number): number => {
  const table: Record<number, number> = { [-1]: 44, 1: 36, 2: 41, 3: 43, 4: 45, 5: 38, 6: 47, 7: 48, 8: 51, 9: 42, 10: 49 };
  return table[pos] ?? (pos < 1 ? 36 : 49);
};

/** Keyboard entry for percussion staves: letter -> GM drum note. */
export const DRUM_KEYS: Record<string, number> = { c: 36, d: 38, e: 42, f: 46, g: 49, a: 51, b: 41 };

/** Notes that are drawn with cross-shaped heads. */
export const DRUM_X = new Set([42, 44, 46, 49, 51, 52, 53, 55, 57, 59]);
