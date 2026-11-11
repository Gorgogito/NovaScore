//! Seguimiento de pulso a partir de los inicios de nota (tiempo en segundos).
//!
//! 1. `estimate_period`: periodo de pulso global por autocorrelación de la señal de inicios
//!    (tolera rubato mejor que un peine de fase fino).
//! 2. `track_beats`: programación dinámica al estilo Ellis (2007): cada pulso cae cerca de
//!    inicios fuertes y a una distancia cercana al periodo.
//! 3. `BeatMap`: tiempo (s) → posición en pulsos, interpolando entre pulsos.

/// Resolución de la señal de inicios (s).
const DT: f64 = 0.01;
/// Desviación típica del suavizado gaussiano de cada inicio (s).
const SIGMA: f64 = 0.03;

/// Señal de inicios muestreada a `DT`: un gaussiano por nota, ponderado por su amplitud.
pub fn onset_signal(onsets: &[(f64, f32)], pad: f64) -> Vec<f32> {
    let end = onsets.iter().map(|o| o.0).fold(0.0, f64::max) + pad;
    let n = (end / DT).ceil() as usize + 1;
    let mut x = vec![0.0f32; n];
    let half = (3.0 * SIGMA / DT).ceil() as i64;
    for &(t, w) in onsets {
        let c = (t / DT).round() as i64;
        for k in -half..=half {
            let i = c + k;
            if i >= 0 && (i as usize) < n {
                let d = k as f64 * DT / SIGMA;
                x[i as usize] += w * (-0.5 * d * d).exp() as f32;
            }
        }
    }
    x
}

fn acf(x: &[f32], lag: usize) -> f64 {
    if lag >= x.len() {
        return 0.0;
    }
    x.iter().zip(&x[lag..]).map(|(a, b)| *a as f64 * *b as f64).sum()
}

/// Periodo de pulso (s) y fiabilidad (0..1) por autocorrelación con prior de tempo.
/// Si el periodo resulta menor de 0,5 s y el doble también es muy periódico, se prefiere el
/// doble: evita notar la corchea como negra (p. ej. 124 en vez de 62 negras/min).
pub fn estimate_period(onsets: &[(f64, f32)]) -> Option<(f64, f64)> {
    if onsets.len() < 8 {
        return None;
    }
    let x = onset_signal(onsets, 4.0);
    let a0 = acf(&x, 0);
    if a0 <= 0.0 {
        return None;
    }
    let ac = |l: usize| acf(&x, l) / a0;
    let prior = |period: f64| (-0.5 * ((60.0 / period / 100.0).log2() / 0.6).powi(2)).exp();
    let (lo, hi) = ((0.3 / DT) as usize, (1.5 / DT) as usize);
    let mut best = (lo, f64::MIN);
    for l in lo..=hi {
        let tps = ac(l) + 0.5 * ac(2 * l) + 0.25 * ac(4 * l);
        let s = tps * prior(l as f64 * DT);
        if s > best.1 {
            best = (l, s);
        }
    }
    let mut l = best.0;
    // refinado parabólico del pico de la autocorrelación
    let mut frac = 0.0;
    if l > 1 {
        let (y0, y1, y2) = (ac(l - 1), ac(l), ac(l + 1));
        let den = y0 - 2.0 * y1 + y2;
        if den.abs() > 1e-12 {
            frac = (0.5 * (y0 - y2) / den).clamp(-0.5, 0.5);
        }
    }
    let mut period = (l as f64 + frac) * DT;
    if period < 0.5 {
        let l2 = (2.0 * period / DT).round() as usize;
        if ac(l2) >= 0.8 * ac(l) {
            l = l2;
            period = l as f64 * DT;
            let (y0, y1, y2) = (ac(l - 1), ac(l), ac(l + 1));
            let den = y0 - 2.0 * y1 + y2;
            if den.abs() > 1e-12 {
                period = (l as f64 + (0.5 * (y0 - y2) / den).clamp(-0.5, 0.5)) * DT;
            }
        }
    }
    Some((period, ac(l).clamp(0.0, 1.0)))
}

/// Tiempos de pulso (s) por programación dinámica. `period` es el periodo esperado y
/// `tightness` cuánto se penaliza apartarse de él (mayor = tempo más rígido).
pub fn track_beats(onsets: &[(f64, f32)], period: f64, tightness: f64) -> Vec<f64> {
    dp(onsets, period, tightness).0
}

/// Programación dinámica: (pulsos en s, puntuación total del mejor camino).
fn dp(onsets: &[(f64, f32)], period: f64, tightness: f64) -> (Vec<f64>, f64) {
    let x = onset_signal(onsets, 2.0 * period);
    let max = x.iter().cloned().fold(0.0f32, f32::max).max(1e-9);
    let o: Vec<f64> = x.iter().map(|v| (*v / max) as f64).collect();
    let n = o.len();
    let t_frames = period / DT;
    let (lo, hi) = ((0.5 * t_frames).round().max(1.0) as usize, (2.0 * t_frames).round() as usize);
    let mut cost = vec![0.0f64; n];
    let mut prev = vec![usize::MAX; n];
    // penalización logarítmica precalculada por distancia
    let pen: Vec<f64> = (0..=hi).map(|d| if d == 0 { 0.0 } else { tightness * (d as f64 / t_frames).ln().powi(2) }).collect();
    for i in 0..n {
        // Sin predecesor (empezar aquí) equivale a 0; solo se enlaza si mejora.
        let mut best = (0.0f64, usize::MAX);
        if i >= lo {
            for d in lo..=hi.min(i) {
                let c = cost[i - d] - pen[d];
                if c > best.0 {
                    best = (c, i - d);
                }
            }
        }
        cost[i] = o[i] + best.0;
        prev[i] = best.1;
    }
    // el último pulso: el mejor entre los fotogramas del último periodo de notas
    let last_onset = (onsets.iter().map(|o| o.0).fold(0.0, f64::max) / DT) as usize;
    let from = last_onset.saturating_sub(t_frames as usize).min(n - 1);
    let mut i = (from..n).max_by(|&a, &b| cost[a].partial_cmp(&cost[b]).unwrap()).unwrap();
    let total = cost[i];
    let mut frames = vec![i];
    while prev[i] != usize::MAX {
        i = prev[i];
        frames.push(i);
    }
    frames.reverse();
    (frames.into_iter().map(|f| f as f64 * DT).collect(), total)
}

/// Puntuación media por pulso del mejor camino con ese periodo (0 si hay menos de 4 pulsos).
pub fn score_at(onsets: &[(f64, f32)], period: f64, tightness: f64) -> f64 {
    let (b, total) = dp(onsets, period, tightness);
    if b.len() >= 4 { total / b.len() as f64 } else { 0.0 }
}

/// Puntuación de cada periodo candidato (s) para diagnóstico: `(periodo, puntuación sin prior, pulsos)`.
pub fn period_scores(onsets: &[(f64, f32)], tightness: f64) -> Vec<(f64, f64, usize)> {
    let mut out = Vec::new();
    let mut p = 0.35;
    while p <= 1.2 {
        let (b, total) = dp(onsets, p, tightness);
        if b.len() >= 4 {
            out.push((p, total / b.len() as f64, b.len()));
        }
        p *= 1.03;
    }
    out
}

/// Penalización del periodo de pulso que equivale a un múltiplo de 3 de la rejilla (tatum).
pub const TATUM_TERNARY_PENALTY: f64 = 0.3;
/// Tolerancia relativa (|n−k| < ventana·k) para considerar el periodo múltiplo de 3 del tatum; con rubato el
/// tatum medido sale algo corto, por lo que la ventana no puede ser estrecha.
pub const TATUM_TERNARY_WINDOW: f64 = 0.12;

/// Unidad mínima de la rejilla rítmica (s): el intervalo entre inicios más corto con peso
/// significativo (≥ 25 % del más frecuente). `None` con muy pocos inicios.
pub fn tatum(onsets: &[(f64, f32)]) -> Option<f64> {
    let mut t: Vec<f64> = onsets.iter().map(|o| o.0).collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut u: Vec<f64> = Vec::new();
    for x in t {
        if u.last().map_or(true, |&l| x - l > 0.04) {
            u.push(x);
        }
    }
    if u.len() < 24 {
        return None;
    }
    let mut hist = vec![0u32; 60]; // casillas de 10 ms entre 60 ms y 660 ms
    for w in u.windows(2) {
        let d = w[1] - w[0];
        if (0.06..0.66).contains(&d) {
            hist[((d - 0.06) / 0.01) as usize] += 1;
        }
    }
    // suaviza con los vecinos para no depender del borde de casilla
    let sm: Vec<u32> = (0..60).map(|i| hist[i] + hist[i.saturating_sub(1)] + hist[(i + 1).min(59)]).collect();
    let max = *sm.iter().max()?;
    if max < 8 {
        return None;
    }
    let i = sm.iter().position(|&c| c as f64 >= 0.25 * max as f64)?;
    // centro de masa local
    let (mut sw, mut s) = (0.0, 0.0);
    for j in i.saturating_sub(1)..=(i + 1).min(59) {
        sw += hist[j] as f64;
        s += hist[j] as f64 * (0.06 + 0.01 * j as f64 + 0.005);
    }
    if sw <= 0.0 {
        return None;
    }
    let t = s / sw;
    // solo si la rejilla es fiable: unidad ≥ 80 ms y al menos el 35 % de los intervalos la igualan
    // (en grabaciones ruidosas la «unidad» sale de 60 ms y casi ningún intervalo la sigue)
    let iois: Vec<f64> = u.windows(2).map(|w| w[1] - w[0]).collect();
    let share = iois.iter().filter(|&&d| (d - t).abs() < 0.2 * t).count() as f64 / iois.len() as f64;
    (t >= 0.08 && share >= 0.35).then_some(t)
}

/// Coincidencia mínima de intervalos para considerar que la textura se repite (las piezas de arpegios dan 0,33-0,35;
/// las melodías y la música sin patrón, ≤ 0,18).
pub const REP_MIN_STRENGTH: f64 = 0.25;
/// Penalización máxima de un periodo de pulso incoherente con la repetición.
pub const REP_PENALTY: f64 = 0.7;

/// Repetición de la textura: la figura (secuencia de alturas) se repite cada `lag` s con fuerza `strength` (0..1).
#[derive(Debug, Clone, Copy)]
pub struct RepHint {
    pub lag: f64,
    pub strength: f64,
}

/// Mayor intervalo de repetición de la secuencia de alturas, medido en NÚMERO DE NOTAS (inmune al rubato);
/// el lapso en segundos es la mediana de `inicio[i+k] − inicio[i]`. `notes` = (inicio, altura).
/// `None` si no hay repetición clara (melodías, música sin patrón).
pub fn repetition(notes: &[(f64, u8)]) -> Option<RepHint> {
    let mut v: Vec<(f64, u8)> = notes.to_vec();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
    if v.len() < 48 {
        return None;
    }
    let max_k = 48.min(v.len() / 3);
    // intervalos entre notas consecutivas: el dibujo de un arpegio se repite aunque cambie la armonía
    let st: Vec<i32> = v.windows(2).map(|w| w[1].1 as i32 - w[0].1 as i32).collect();
    let m: Vec<f64> = (1..=max_k)
        .map(|k| (0..st.len() - k).filter(|&i| st[i] == st[i + k]).count() as f64 / (st.len() - k) as f64)
        .collect();
    let best = m.iter().cloned().fold(0.0, f64::max);
    if best < REP_MIN_STRENGTH {
        return None;
    }
    // el menor k cuya coincidencia casi iguala a la mejor (los múltiplos del periodo también coinciden)
    let k = m.iter().position(|&x| x >= 0.9 * best).unwrap() + 1;
    if k < 2 {
        return None; // nota repetida: no informa del pulso
    }
    let mut d: Vec<f64> = (0..v.len() - k).map(|i| v[i + k].0 - v[i].0).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(RepHint { lag: d[d.len() / 2], strength: best })
}

/// Busca el pulso sin periodo de partida fiable: prueba periodos candidatos (≈ 50–170
/// negras/min), puntúa cada camino (fuerza media de inicio por pulso × prior de tempo) y
/// refina el mejor. Con `seed` se parte de ese periodo. Si el elegido supera las 120 negras/min
/// y el doble puntúa casi igual, se prefiere el doble (evita notar la corchea como negra).
pub fn find_beats(onsets: &[(f64, f32)], seed: Option<f64>, tightness: f64) -> Vec<f64> {
    find_beats_hint(onsets, seed, tightness, None)
}

/// Factor (≤ 1) que penaliza un periodo de pulso que no divide el lapso de repetición de la textura en un número entero
/// de pulsos (p. ej. un pulso de 3 semicorcheas con una figura de 8: 2,67 pulsos por repetición).
pub fn repetition_coherence(hint: Option<RepHint>, period: f64) -> f64 {
    match hint {
        Some(h) if h.lag > 0.0 && period > 0.0 => {
            let r = if h.lag >= period { h.lag / period } else { period / h.lag };
            let n = r.round().max(1.0);
            let dev = (r - n).abs() / n;
            1.0 - REP_PENALTY * ((dev - 0.04) / 0.12).clamp(0.0, 1.0)
        }
        _ => 1.0,
    }
}

/// Como [`find_beats`], con la repetición de la textura: un periodo de pulso que no divide la figura en un
/// número entero de pulsos (p. ej. 3 semicorcheas con una figura de 8) se penaliza según la fuerza de la repetición.
pub fn find_beats_hint(onsets: &[(f64, f32)], seed: Option<f64>, tightness: f64, hint: Option<RepHint>) -> Vec<f64> {
    let prior = |period: f64| (-0.5 * ((60.0 / period / 100.0).log2() / 0.6).powi(2)).exp();
    // En texturas uniformes (arpegios de semicorcheas) el pulso es ambiguo entre 3 y 4 divisiones
    // de la rejilla; sin evidencia de compuesto se prefiere un múltiplo binario de la rejilla.
    let tat = tatum(onsets);
    let lambda = TATUM_TERNARY_PENALTY;
    let ternary = |period: f64| -> f64 {
        match tat {
            Some(t) => {
                let n = period / t;
                let k = n.round();
                if k >= 3.0 && (n - k).abs() < TATUM_TERNARY_WINDOW * k && (k as u32) % 3 == 0 { 1.0 - lambda } else { 1.0 }
            }
            None => 1.0,
        }
    };
    let score = |period: f64| -> Option<f64> {
        let (b, total) = dp(onsets, period, tightness);
        (b.len() >= 4).then(|| total / b.len() as f64 * prior(period) * ternary(period) * repetition_coherence(hint, period))
    };
    let period = match seed {
        Some(p) => p,
        None => {
            let mut best = (0.6, f64::MIN);
            let mut p = 0.35;
            while p <= 1.2 {
                if let Some(s) = score(p) {
                    if s > best.1 {
                        best = (p, s);
                    }
                }
                p *= 1.03;
            }
            if best.0 < 0.5 {
                if let (Some(s1), Some(s2)) = (score(best.0), score(best.0 * 2.0)) {
                    if s2 >= 0.9 * s1 {
                        best.0 *= 2.0;
                    }
                }
            }
            best.0
        }
    };
    track_beats_refined(onsets, period, tightness)
}

/// Como [`track_beats`], pero reajusta el periodo esperado a la mediana de los intervalos
/// hallados y repite (hasta 4 pasadas): corrige un periodo de partida algo desviado, que de
/// otro modo hace saltarse o duplicar pulsos.
pub fn track_beats_refined(onsets: &[(f64, f32)], period: f64, tightness: f64) -> Vec<f64> {
    let mut p = period;
    let mut beats = track_beats(onsets, p, tightness);
    for _ in 0..3 {
        if beats.len() < 4 {
            break;
        }
        let mut d: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = d[d.len() / 2];
        if (med - p).abs() / p < 0.005 {
            break;
        }
        p = med;
        beats = track_beats(onsets, p, tightness);
    }
    beats
}

/// Correspondencia tiempo (s) → posición en pulsos (el pulso `k` está en `beats[k]`).
#[derive(Debug, Clone)]
pub struct BeatMap {
    beats: Vec<f64>,
}

impl BeatMap {
    /// Mapa de tempo constante: pulsos `phase + k·beat` cubriendo `[0, end]`.
    pub fn regular(phase: f64, beat: f64, end: f64) -> BeatMap {
        let mut beats = Vec::new();
        let mut t = phase - 2.0 * beat;
        while t <= end + 2.0 * beat {
            beats.push(t);
            t += beat;
        }
        BeatMap { beats }
    }

    /// Mapa a partir de pulsos seguidos; se añaden dos pulsos extrapolados a cada lado.
    pub fn from_beats(mut beats: Vec<f64>, fallback_period: f64) -> BeatMap {
        if beats.len() < 2 {
            let t = beats.first().copied().unwrap_or(0.0);
            return BeatMap::regular(t, fallback_period, t + 10.0 * fallback_period);
        }
        let (p0, p1) = (beats[1] - beats[0], beats[beats.len() - 1] - beats[beats.len() - 2]);
        for _ in 0..2 {
            beats.insert(0, beats[0] - p0);
        }
        for _ in 0..2 {
            let l = *beats.last().unwrap();
            beats.push(l + p1);
        }
        BeatMap { beats }
    }

    /// Posición en pulsos (puede ser negativa antes del primer pulso).
    pub fn pos(&self, t: f64) -> f64 {
        let b = &self.beats;
        let i = match b.binary_search_by(|x| x.partial_cmp(&t).unwrap()) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let i = i.min(b.len() - 2);
        i as f64 + (t - b[i]) / (b[i + 1] - b[i])
    }

    pub fn beats(&self) -> &[f64] {
        &self.beats
    }

    /// Tempo mediano (negras/min).
    pub fn median_bpm(&self) -> f64 {
        let mut d: Vec<f64> = self.beats.windows(2).map(|w| w[1] - w[0]).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        60.0 / d[d.len() / 2]
    }
}

#[cfg(test)]
mod tests {
    /// Arpegio: figura de 8 semicorcheas cuya altura base cambia cada compás (la armonía), a `bpm`.
    fn arpeggio(bars: usize, bpm: f64) -> Vec<(f64, u8)> {
        let u = 60.0 / bpm / 4.0;
        let fig = [0u8, 4, 7, 12, 16, 12, 7, 4];
        let roots = [48u8, 50, 43, 48];
        let mut v = Vec::new();
        for b in 0..bars {
            for i in 0..16 {
                v.push(((b * 16 + i) as f64 * u, roots[b % 4] + fig[i % 8]));
            }
        }
        v
    }

    #[test]
    fn repetition_finds_the_figure_in_arpeggios_but_not_in_a_melody() {
        let h = repetition(&arpeggio(16, 72.0)).expect("el arpegio se repite");
        let u = 60.0 / 72.0 / 4.0;
        assert!(h.strength > REP_MIN_STRENGTH);
        assert!((h.lag / u - 8.0).abs() < 0.5, "lapso {} (8 semicorcheas = {})", h.lag, 8.0 * u);
        // melodía sin patrón: pseudoaleatoria
        let mut seed = 7u32;
        let melody: Vec<(f64, u8)> = (0..200)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (i as f64 * 0.3, 55 + (seed >> 24) as u8 % 20)
            })
            .collect();
        assert!(repetition(&melody).map_or(true, |h| h.strength < REP_MIN_STRENGTH));
    }

    #[test]
    fn coherence_penalizes_periods_that_do_not_divide_the_repetition() {
        let h = Some(RepHint { lag: 1.6, strength: 0.33 });
        assert_eq!(repetition_coherence(h, 0.8), 1.0); // 2 pulsos
        assert_eq!(repetition_coherence(h, 0.4), 1.0); // 4 pulsos
        assert_eq!(repetition_coherence(h, 1.6), 1.0); // 1 pulso
        let c = repetition_coherence(h, 0.6); // 2,67 pulsos por repetición
        assert!(c < 0.7 && c > 1.0 - REP_PENALTY, "{c}");
        assert!(repetition_coherence(h, 0.5) > c); // 3,2 pulsos: menos incoherente
        assert!((repetition_coherence(h, 1.1) - (1.0 - REP_PENALTY)).abs() < 1e-9); // 1,45 pulsos: la mayor desviación
        assert_eq!(repetition_coherence(None, 0.6), 1.0);
    }

    #[test]
    fn tatum_of_uniform_sixteenths_and_noise() {
        let uniform: Vec<(f64, f32)> = (0..80).map(|i| (1.0 + i as f64 * 0.21, 1.0)).collect();
        let t = tatum(&uniform).expect("rejilla");
        assert!((t - 0.21).abs() < 0.02, "{t}");
        // intervalos dispersos: sin rejilla fiable
        let mut x = 7u64;
        let noisy: Vec<(f64, f32)> = (0..80)
            .scan(0.0, |a, _| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *a += 0.07 + (x >> 40) as f64 / (1u64 << 24) as f64 * 0.5;
                Some((*a, 1.0))
            })
            .collect();
        assert!(tatum(&noisy).is_none());
    }

    use super::*;

    /// Inicios en cada pulso de un tempo que cambia: 100 → 120 negras/min en 40 pulsos.
    fn accelerando() -> (Vec<(f64, f32)>, Vec<f64>) {
        let mut t = 1.0;
        let (mut on, mut truth) = (Vec::new(), Vec::new());
        for k in 0..40 {
            let bpm = 100.0 + 20.0 * k as f64 / 40.0;
            truth.push(t);
            on.push((t, 1.0f32));
            on.push((t + 30.0 / bpm, 0.5)); // contratiempo más débil
            t += 60.0 / bpm;
        }
        (on, truth)
    }

    #[test]
    fn beat_map_interpolates_between_beats() {
        let m = BeatMap::from_beats(vec![0.0, 1.0, 2.5, 3.5], 1.0);
        // los dos pulsos añadidos a cada lado desplazan el índice: pulso 0 real = posición 2
        assert!((m.pos(0.0) - 2.0).abs() < 1e-9);
        assert!((m.pos(1.75) - 3.5).abs() < 1e-9);
        assert!((m.pos(-1.0) - 1.0).abs() < 1e-9);
        assert!((m.median_bpm() - 60.0).abs() < 1.0);
    }

    #[test]
    fn tracker_follows_an_accelerando() {
        let (on, truth) = accelerando();
        let beats = track_beats_refined(&on, 0.55, 8.0);
        // cada pulso real tiene un pulso seguido a menos de 40 ms
        let missed = truth.iter().filter(|t| !beats.iter().any(|b| (b - *t).abs() < 0.04)).count();
        assert!(missed <= 2, "{missed} pulsos sin seguir de {}", truth.len());
    }

    #[test]
    fn find_beats_chooses_the_quarter_not_the_offbeat() {
        let (on, _) = accelerando();
        let b = find_beats(&on, None, 8.0);
        let m = BeatMap::from_beats(b, 0.6);
        let bpm = m.median_bpm();
        assert!((95.0..125.0).contains(&bpm), "{bpm}");
    }
}
