//! Transcripción de piano de alta precisión: «High-resolution Piano Transcription» (Kong et al.,
//! ByteDance). Código Apache-2.0; pesos CC-BY-4.0 (Zenodo 4034264) convertidos a ONNX con
//! `scripts/export-piano-onnx.py`. El modelo (~98 MB) no va en el binario: se descarga bajo demanda.
//!
//! Entrada: audio mono a 16 kHz en segmentos de 10 s (el ONNX incluye STFT y mel). Salidas por
//! fotograma (100/s) y nota: regresión de inicio y fin, actividad y velocidad. El
//! postprocesado es un port del de `piano_transcription_inference`.
use crate::NoteEvent;
use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const PIANO_RATE: usize = 16000;
const SEGMENT: usize = PIANO_RATE * 10;
const FPS: f64 = 100.0;
const BEGIN_NOTE: u8 = 21;
const N: usize = 88;

/// Umbrales del postprocesado (valores del repositorio original).
#[derive(Debug, Clone, Copy)]
pub struct PianoParams {
    pub onset: f32,
    pub offset: f32,
    pub frame: f32,
}

impl Default for PianoParams {
    fn default() -> Self {
        PianoParams { onset: 0.3, offset: 0.3, frame: 0.1 }
    }
}

/// Dispositivo de inferencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// GPU (DirectML) si está disponible; si no, CPU.
    Auto,
    Cpu,
}

struct Loaded {
    path: PathBuf,
    device: Device,
    session: Session,
}

static SESSION: Mutex<Option<Loaded>> = Mutex::new(None);

fn load(path: &Path, device: Device) -> Result<Session, String> {
    use ort::ep::{DirectML, ExecutionProvider, CPU};
    let env = crate::environment()?;
    let mut b = Session::builder(env).map_err(|e| format!("Sesión: {e}"))?;
    if device == Device::Auto {
        if DirectML::is_available().unwrap_or(false) {
            b = b.with_execution_providers([DirectML::default().build()]).map_err(|e| format!("GPU: {e}"))?;
        }
    } else {
        b = b.with_execution_providers([CPU::default().build()]).map_err(|e| format!("CPU: {e}"))?;
    }
    b.commit_from_file(path).map_err(|e| format!("No se pudo cargar el modelo de piano ({}): {e}", path.display()))
}

/// Salidas del modelo ya unidas: cada matriz tiene `frames × 88`.
pub struct Activations {
    pub onset: Vec<f32>,
    pub offset: Vec<f32>,
    pub frame: Vec<f32>,
    pub velocity: Vec<f32>,
    pub frames: usize,
}

/// Audio mono a 16 kHz → activaciones.
pub fn infer(model: &Path, device: Device, samples: &[f32], batch: usize) -> Result<Activations, String> {
    let mut guard = SESSION.lock().map_err(|_| "Sesión bloqueada".to_string())?;
    if guard.as_ref().map_or(true, |l| l.path != model || l.device != device) {
        *guard = Some(Loaded { path: model.to_path_buf(), device, session: load(model, device)? });
    }
    let session = &mut guard.as_mut().unwrap().session;

    let len = samples.len();
    let padded_len = len.div_ceil(SEGMENT).max(1) * SEGMENT;
    let mut x = samples.to_vec();
    x.resize(padded_len, 0.0);
    let mut starts = Vec::new();
    let mut p = 0;
    while p + SEGMENT <= padded_len {
        starts.push(p);
        p += SEGMENT / 2;
    }
    // salidas por segmento: 1001 fotogramas (el último sobra: center=True)
    let seg_frames = SEGMENT / (PIANO_RATE / 100) + 1;
    let mut seg_out: Vec<[Vec<f32>; 4]> = Vec::new();
    for chunk in starts.chunks(batch.max(1)) {
        let mut data = Vec::with_capacity(chunk.len() * SEGMENT);
        for &s in chunk {
            data.extend_from_slice(&x[s..s + SEGMENT]);
        }
        let input = Tensor::from_array(([chunk.len(), SEGMENT], data)).map_err(|e| format!("Entrada: {e}"))?;
        let outputs = session.run(ort::inputs!["audio" => input]).map_err(|e| format!("Inferencia: {e}"))?;
        let get = |name: &str| -> Result<Vec<f32>, String> {
            let (shape, d) = outputs[name].try_extract_tensor::<f32>().map_err(|e| format!("Salida {name}: {e}"))?;
            if shape.len() != 3 || shape[1] as usize != seg_frames || shape[2] as usize != N {
                return Err(format!("Forma de salida inesperada: {shape:?}"));
            }
            Ok(d.to_vec())
        };
        let (on, off, fr, ve) = (get("onset")?, get("offset")?, get("frame")?, get("velocity")?);
        let per = seg_frames * N;
        for i in 0..chunk.len() {
            seg_out.push([
                on[i * per..(i + 1) * per].to_vec(),
                off[i * per..(i + 1) * per].to_vec(),
                fr[i * per..(i + 1) * per].to_vec(),
                ve[i * per..(i + 1) * per].to_vec(),
            ]);
        }
    }
    // deframe: quitar el fotograma sobrante y conservar 0-75 % / 25-75 % / 25-100 %
    let f = seg_frames - 1;
    let mut res: [Vec<f32>; 4] = Default::default();
    for (k, r) in res.iter_mut().enumerate() {
        let n = seg_out.len();
        for (i, s) in seg_out.iter().enumerate() {
            let s = &s[k][..f * N];
            if n == 1 {
                r.extend_from_slice(s);
            } else if i == 0 {
                r.extend_from_slice(&s[..f * 3 / 4 * N]);
            } else if i == n - 1 {
                r.extend_from_slice(&s[f / 4 * N..]);
            } else {
                r.extend_from_slice(&s[f / 4 * N..f * 3 / 4 * N]);
            }
        }
    }
    let frames = ((len as f64 / PIANO_RATE as f64 * FPS) as usize + 1).min(res[0].len() / N);
    for r in res.iter_mut() {
        r.truncate(frames * N);
    }
    let [onset, offset, frame, velocity] = res;
    Ok(Activations { onset, offset, frame, velocity, frames })
}

/// Picos de la regresión de inicio/fin: devuelve (binario, desplazamiento subfotograma).
fn binarize(x: &[f32], frames: usize, k: usize, threshold: f32, neighbour: usize) -> (Vec<bool>, Vec<f32>) {
    let at = |n: usize| x[n * N + k];
    let mut bin = vec![false; frames];
    let mut shift = vec![0.0f32; frames];
    if frames <= 2 * neighbour {
        return (bin, shift);
    }
    for n in neighbour..frames - neighbour {
        if at(n) > threshold {
            let mut mono = true;
            for i in 0..neighbour {
                if at(n - i) < at(n - i - 1) || at(n + i) < at(n + i + 1) {
                    mono = false;
                }
            }
            if mono {
                bin[n] = true;
                shift[n] = if at(n - 1) > at(n + 1) {
                    (at(n + 1) - at(n - 1)) / (at(n) - at(n + 1)) / 2.0
                } else {
                    (at(n + 1) - at(n - 1)) / (at(n) - at(n - 1)) / 2.0
                };
            }
        }
    }
    (bin, shift)
}

/// Activaciones → notas (port de `note_detection_with_onset_offset_regress`).
pub fn decode(a: &Activations, p: &PianoParams) -> Vec<NoteEvent> {
    let mut out: Vec<NoteEvent> = Vec::new();
    for k in 0..N {
        let (on, on_shift) = binarize(&a.onset, a.frames, k, p.onset, 2);
        let (off, off_shift) = binarize(&a.offset, a.frames, k, p.offset, 4);
        let frame = |i: usize| a.frame[i * N + k];
        let vel = |i: usize| a.velocity[i * N + k];
        let push = |b: usize, fin: usize, off_s: f32, out: &mut Vec<NoteEvent>| {
            let start = (b as f64 + on_shift[b] as f64) / FPS;
            out.push(NoteEvent {
                start,
                end: ((fin as f64 + off_s as f64) / FPS).max(start + 0.02),
                pitch: BEGIN_NOTE + k as u8,
                amplitude: vel(b),
                onset: true,
            });
        };
        let mut bgn: Option<usize> = None;
        let mut disappear: Option<usize> = None;
        let mut occur: Option<usize> = None;
        for i in 0..a.frames {
            if on[i] {
                if let Some(b) = bgn {
                    push(b, i.saturating_sub(1), 0.0, &mut out);
                    disappear = None;
                    occur = None;
                }
                bgn = Some(i);
            }
            let Some(b) = bgn else { continue };
            if i <= b {
                continue;
            }
            if frame(i) <= p.frame && disappear.is_none() {
                disappear = Some(i);
            }
            if off[i] && occur.is_none() {
                occur = Some(i);
            }
            if let Some(d) = disappear {
                let fin = match occur {
                    Some(o) if o - b > d - o => o,
                    _ => d,
                };
                push(b, fin, off_shift[fin], &mut out);
                bgn = None;
                disappear = None;
                occur = None;
            } else if i - b >= 600 || i == a.frames - 1 {
                push(b, i, off_shift[i], &mut out);
                bgn = None;
                disappear = None;
                occur = None;
            }
        }
    }
    out.sort_by(|x, y| x.start.partial_cmp(&y.start).unwrap().then(x.pitch.cmp(&y.pitch)));
    out
}

/// Audio mono a 16 kHz → notas de piano.
pub fn transcribe_samples(model: &Path, device: Device, samples: &[f32], p: &PianoParams) -> Result<Vec<NoteEvent>, String> {
    let a = infer(model, device, samples, 4)?;
    Ok(decode(&a, p))
}

/// Archivo de audio → notas de piano.
pub fn transcribe_file(model: &Path, device: Device, path: &Path, p: &PianoParams) -> Result<Vec<NoteEvent>, String> {
    let (mono, rate) = crate::audio::decode_mono(path)?;
    let x = crate::audio::resample(&mono, rate, PIANO_RATE as u32);
    transcribe_samples(model, device, &x, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Activaciones sintéticas: una nota (La4 = 69 → columna 48) de 0,50 s a 1,00 s con pico de inicio
    /// y de fin triangulares, y otra nota repetida sin silencio entre medias.
    #[test]
    fn decode_places_notes_and_splits_repeats() {
        let frames = 300;
        let k = 69 - BEGIN_NOTE as usize;
        let mut a = Activations { onset: vec![0.0; frames * N], offset: vec![0.0; frames * N], frame: vec![0.0; frames * N], velocity: vec![0.0; frames * N], frames };
        let peak = |v: &mut Vec<f32>, c: usize| {
            for (d, h) in [(-2i32, 0.3f32), (-1, 0.6), (0, 1.0), (1, 0.6), (2, 0.3)] {
                v[(c as i32 + d) as usize * N + k] = h;
            }
        };
        peak(&mut a.onset, 50);
        peak(&mut a.offset, 100);
        peak(&mut a.onset, 150); // repetición: corta la nota anterior sin offset
        for i in 50..=100 {
            a.frame[i * N + k] = 0.9;
        }
        for i in 150..=200 {
            a.frame[i * N + k] = 0.9;
        }
        for i in 0..frames {
            a.velocity[i * N + k] = 0.7;
        }
        let notes = decode(&a, &PianoParams::default());
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().all(|n| n.pitch == 69 && (n.amplitude - 0.7).abs() < 1e-6));
        assert!((notes[0].start - 0.50).abs() < 0.02 && (notes[0].end - 1.0).abs() < 0.03, "{:?}", (notes[0].start, notes[0].end));
        assert!((notes[1].start - 1.50).abs() < 0.02);
    }
}
