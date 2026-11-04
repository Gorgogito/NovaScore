import { MIDIControllers } from "spessasynth_core";
import { WorkletSynthesizer } from "spessasynth_lib";
import processorUrl from "spessasynth_lib/dist/spessasynth_processor.min.js?url";
import type { Playback } from "./api";
import { measureLen, Score } from "./types";

const SOUNDFONT_URL = "/soundfonts/MuseScore_General.sf3";
const LOOKAHEAD = 0.35; // seconds of audio scheduled ahead

interface Ev {
  at: number; // seconds from playback start
  on: boolean;
  ch: number;
  midi: number;
  vel: number;
  /** Sustain pedal value (0-127) instead of a note. */
  pedal?: number;
}

/** SoundFont playback (MuseScore_General, MIT) through SpessaSynth (Apache-2.0). */
export class Player {
  private ctx: AudioContext | null = null;
  private synth: WorkletSynthesizer | null = null;
  private loading: Promise<void> | null = null;
  private raf = 0;
  private timer = 0;
  private channels = new Set<number>();

  /** Load the synthesizer and SoundFont once. Safe to call repeatedly. */
  init(): Promise<void> {
    this.loading ??= (async () => {
      const ctx = new AudioContext();
      await ctx.audioWorklet.addModule(processorUrl);
      const synth = new WorkletSynthesizer(ctx);
      synth.connect(ctx.destination);
      const res = await fetch(SOUNDFONT_URL);
      if (!res.ok) throw new Error(`No se pudo cargar el SoundFont (${res.status})`);
      await synth.soundBankManager.addSoundBank(await res.arrayBuffer(), "main");
      await synth.isReady;
      this.ctx = ctx;
      this.synth = synth;
    })().catch((e) => {
      this.loading = null;
      throw e;
    });
    return this.loading;
  }

  /** `sounds` come from the Rust core with ties already merged. */
  async play(score: Score, { sounds, channels, pedals, order }: Playback, fromMeasure: number, onTick: (key: string | null) => void, onEnd: () => void) {
    this.stop();
    await this.init();
    const ctx = this.ctx!;
    const synth = this.synth!;
    await ctx.resume();

    // Playing time: measures in the unfolded order (repeats and voltas), each as long as it is.
    const offsets: number[] = [];
    let acc = 0;
    for (const mi of order) {
      offsets.push(acc);
      acc += measureLen(score, mi);
    }
    const fromIdx = Math.max(0, order.indexOf(fromMeasure));
    const secPerTick = 60 / (score.tempo * 480);
    const base = offsets[fromIdx] ?? 0;

    score.parts.forEach((p, i) => {
      if (!p.percussion) synth.programChange(channels[i], p.program);
      synth.controllerChange(channels[i], MIDIControllers.mainVolume, p.volume);
    });

    const evs: Ev[] = [];
    let endSec = 0;
    for (const s of sounds) {
      if (s.start < base) continue;
      const ch = s.channel;
      const at = (s.start - base) * secPerTick;
      const off = at + s.ticks * secPerTick * 0.98;
      evs.push({ at, on: true, ch, midi: s.midi, vel: s.velocity }, { at: off, on: false, ch, midi: s.midi, vel: 0 });
      endSec = Math.max(endSec, off);
      this.channels.add(ch);
    }
    // Pedal: keep the state from before the start position, then follow the changes.
    const pedalState = new Map<number, boolean>();
    for (const p of pedals) {
      if (p.at < base) {
        pedalState.set(p.channel, p.down);
        continue;
      }
      evs.push({ at: (p.at - base) * secPerTick, on: false, ch: p.channel, midi: 0, vel: 0, pedal: p.down ? 127 : 0 });
      this.channels.add(p.channel);
    }
    for (const [ch, down] of pedalState) if (down) evs.push({ at: 0, on: false, ch, midi: 0, vel: 0, pedal: 127 });
    evs.sort((a, b) => a.at - b.at || Number(a.on) - Number(b.on));

    const timeline: { at: number; key: string }[] = [];
    for (let k = fromIdx; k < order.length; k++) {
      const mi = order[k];
      for (const e of score.parts[0].measures[mi]?.events ?? []) timeline.push({ at: (offsets[k] + e.start - base) * secPerTick, key: `${mi}:${e.start}` });
    }

    const t0 = ctx.currentTime + 0.1;
    let next = 0;
    const pump = () => {
      const horizon = ctx.currentTime - t0 + LOOKAHEAD;
      while (next < evs.length && evs[next].at <= horizon) {
        const e = evs[next++];
        const time = t0 + e.at;
        if (e.pedal !== undefined) synth.controllerChange(e.ch, MIDIControllers.sustainPedal, e.pedal, { time });
        else if (e.on) synth.noteOn(e.ch, e.midi, e.vel, { time });
        else synth.noteOff(e.ch, e.midi, { time });
      }
    };
    pump();
    this.timer = window.setInterval(pump, 40);

    let last = -1;
    const loop = () => {
      const t = ctx.currentTime - t0;
      let k = -1;
      for (let i = 0; i < timeline.length && timeline[i].at <= t; i++) k = i;
      if (k !== last) {
        last = k;
        onTick(k >= 0 ? timeline[k].key : null);
      }
      if (t > endSec + 0.4) {
        this.stop();
        onEnd();
        return;
      }
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  stop() {
    cancelAnimationFrame(this.raf);
    clearInterval(this.timer);
    this.raf = 0;
    this.timer = 0;
    for (const ch of this.channels) this.synth?.controllerChange(ch, MIDIControllers.sustainPedal, 0);
    this.synth?.stopAll(true);
    this.channels.clear();
  }
}
