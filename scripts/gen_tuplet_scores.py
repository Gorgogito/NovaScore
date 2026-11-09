#!/usr/bin/env python3
"""Genera partituras MusicXML con verdad exacta para calibrar grupos irregulares (banco eval_scores).

- Tresillos.musicxml: línea de violín de los compases 1-8 de «Tresillos.pdf» (transcrita a mano).
- Sintetica-<n>.musicxml: melodías en 4/4 con un grupo irregular (3, 5 o 6 por tiempo) en una fracción
  de los tiempos; semilla fija. Una voz melódica + bajo de negras (da el pulso).
Uso: python scripts/gen_tuplet_scores.py carpeta_salida
"""
import random
import sys
from pathlib import Path

DIV = 120  # divisiones por negra (múltiplo de 3, 4 y 5)
STEPS = {0: ("C", 0), 2: ("D", 0), 4: ("E", 0), 5: ("F", 0), 7: ("G", 0), 9: ("A", 0), 11: ("B", 0)}
TYPE = {4: "whole", 2: "half", 1: "quarter", 0.5: "eighth", 0.25: "16th", 0.125: "32nd"}


def pitch_xml(midi):
    pc, octv = midi % 12, midi // 12 - 1
    step, alt = STEPS[pc]
    return f"<pitch><step>{step}</step><octave>{octv}</octave></pitch>"


def note(midi, beats, tup=None, voice=1, staff=1, chord=False):
    """beats: duración sonora en negras. tup=(n, normal, escrita_en_negras) para grupos irregulares."""
    dur = round(beats * DIV)
    x = "<note>" + ("<chord/>" if chord else "")
    x += pitch_xml(midi) if midi else "<rest/>"
    x += f"<duration>{dur}</duration><voice>{voice}</voice>"
    if tup:
        n, m, written = tup
        x += f"<type>{TYPE[written]}</type><time-modification><actual-notes>{n}</actual-notes><normal-notes>{m}</normal-notes></time-modification>"
    else:
        x += f"<type>{TYPE[beats]}</type>"
    return x + "</note>"


def wrap(title, parts, fifths=0, tempo=100):
    pl = "".join(f'<score-part id="P{i+1}"><part-name>{n}</part-name></score-part>' for i, (n, _) in enumerate(parts))
    body = ""
    for i, (_, measures) in enumerate(parts):
        body += f'<part id="P{i+1}">'
        for k, m in enumerate(measures):
            attrs = ""
            if k == 0:
                clef = "<clef><sign>G</sign><line>2</line></clef>" if i == 0 else "<clef><sign>F</sign><line>4</line></clef>"
                attrs = f"<attributes><divisions>{DIV}</divisions><key><fifths>{fifths}</fifths></key><time><beats>4</beats><beat-type>4</beat-type></time>{clef}</attributes>"
                if i == 0:
                    attrs += f'<direction placement="above"><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>{tempo}</per-minute></metronome></direction-type><sound tempo="{tempo}"/></direction>'
            body += f'<measure number="{k+1}">{attrs}{"".join(m)}</measure>'
        body += "</part>"
    return (
        '<?xml version="1.0" encoding="UTF-8"?><score-partwise version="3.1"><work><work-title>' + title + "</work-title></work>"
        f"<part-list>{pl}</part-list>{body}</score-partwise>"
    )


def tresillos():
    C5, B4, A4, G4, F4, E4, D4 = 72, 71, 69, 67, 65, 64, 62
    t = lambda a, b, c: [note(p, 1 / 3, (3, 2, 0.5)) for p in (a, b, c)]
    q = lambda p: [note(p, 1)]
    bars = [
        q(C5) + q(G4) + q(C5) + t(B4, A4, G4),
        q(A4) + q(E4) + q(A4) + t(G4, F4, E4),
        q(F4) + q(C5) + q(A4) + t(G4, F4, E4),
        q(G4) + q(G4) + [note(C5, 2)],
        t(B4, A4, G4) + q(C5) + q(G4) + q(C5),
        t(G4, F4, E4) + q(A4) + q(E4) + q(A4),
        t(F4, E4, D4) + q(F4) + q(G4) + q(G4),
        [note(C5, 4)],
    ]
    return wrap("Tresillos (violín, c. 1-8)", [("Violin", bars)], 0, 120)


def sintetica(kind, seed, nbars=32, p=0.3, key=0):
    """kind: 3 → tresillo de corcheas (3:2), 5 → quintillo de semicorcheas (5:4), 6 → sextillo de semicorcheas (6:4)."""
    rnd = random.Random(seed)
    scale = [0, 2, 4, 5, 7, 9, 11]
    deg = 4

    def pick():
        nonlocal deg
        deg = max(0, min(14, deg + rnd.choice([-2, -1, -1, 0, 1, 1, 2])))
        return 60 + key + scale[deg % 7] + 12 * (deg // 7)

    mel, bass = [], []
    for _ in range(nbars):
        m = []
        beat = 0
        while beat < 4:
            if rnd.random() < p:  # un tiempo con grupo irregular
                if kind == 3:
                    m += [note(pick(), 1 / 3, (3, 2, 0.5)) for _ in range(3)]
                elif kind == 5:
                    m += [note(pick(), 1 / 5, (5, 4, 0.25)) for _ in range(5)]
                else:
                    m += [note(pick(), 1 / 6, (6, 4, 0.25)) for _ in range(6)]
                beat += 1
            else:
                r = rnd.random()
                if r < 0.45 or beat == 3:
                    m.append(note(pick(), 1)); beat += 1
                elif r < 0.85:
                    m += [note(pick(), 0.5), note(pick(), 0.5)]; beat += 1
                else:
                    m.append(note(pick(), 2)); beat += 2
        mel.append(m)
        bass.append([note(48 + key, 1) for _ in range(4)])
    return wrap(f"Sintética {kind}", [("Melodia", mel), ("Bajo", bass)], 0, 100)


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    out.mkdir(parents=True, exist_ok=True)
    (out / "Tresillos.musicxml").write_text(tresillos(), encoding="utf-8")
    for kind in (3, 5, 6):
        for i, seed in enumerate((1, 2)):
            (out / f"Sintetica-{kind}-{i+1}.musicxml").write_text(sintetica(kind, seed, p=0.3 if i == 0 else 0.5), encoding="utf-8")
    print("generados en", out)


main()
