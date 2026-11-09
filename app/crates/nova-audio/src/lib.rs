//! Transcripción de audio a notas con Basic Pitch (Spotify, Apache-2.0).
//! Modelo ONNX (225 KB) incluido en el binario; inferencia con ONNX Runtime.
//! Ver `models/LICENSE-basic-pitch.txt`.

mod audio;
pub mod notes;
pub mod piano;

pub use notes::{NoteEvent, Params};

use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const SAMPLE_RATE: usize = 22050;
pub const FFT_HOP: usize = 256;
pub const N_PITCHES: usize = 88;
/// Muestras por ventana de entrada del modelo (2 s − un salto).
pub const WINDOW_SAMPLES: usize = 43844;
/// Fotogramas de salida por ventana.
pub const MODEL_FRAMES_PER_WINDOW: usize = 172;
/// Fotogramas de solapamiento entre ventanas (se recorta la mitad por cada lado).
pub const OVERLAP_FRAMES: usize = 30;

static MODEL: &[u8] = include_bytes!("../models/basic_pitch.onnx");

const IN_NAME: &str = "serving_default_input_2:0";
const OUT_NOTE: &str = "StatefulPartitionedCall:1";
const OUT_ONSET: &str = "StatefulPartitionedCall:2";

struct Engine {
    // El entorno debe vivir tanto como la sesión; ONNX Runtime admite uno solo por proceso.
    _env: ort::environment::Environment,
    session: Mutex<Session>,
}

static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();

/// Entorno de ONNX Runtime compartido (solo se admite uno por proceso).
pub(crate) fn environment() -> Result<&'static ort::environment::Environment, String> {
    engine().map(|e| &e._env)
}

fn engine() -> Result<&'static Engine, String> {
    ENGINE
        .get_or_init(|| {
            let env = ort::environment::init()
                .with_name("novascore")
                .build()
                .map_err(|err| format!("No se pudo iniciar ONNX Runtime: {err}"))?;
            let session = Session::builder(&env)
                .and_then(|mut b| b.commit_from_memory(MODEL))
                .map_err(|err| format!("No se pudo cargar el modelo: {err}"))?;
            Ok(Engine { _env: env, session: Mutex::new(session) })
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// Activaciones del modelo ya unidas: (notas, inicios), con `floor(muestras/256)` filas.
pub fn infer(samples: &[f32]) -> Result<(notes::Matrix, notes::Matrix), String> {
    let e = |s: &str, err: &dyn std::fmt::Display| format!("{s}: {err}");
    let mut session = engine()?.session.lock().map_err(|_| "Sesión del modelo bloqueada".to_string())?;

    let olap_samples = OVERLAP_FRAMES * FFT_HOP;
    let hop = WINDOW_SAMPLES - olap_samples;
    let mut padded = vec![0.0f32; olap_samples / 2];
    padded.extend_from_slice(samples);

    let cut = OVERLAP_FRAMES / 2;
    let mut note_rows: Vec<f32> = Vec::new();
    let mut onset_rows: Vec<f32> = Vec::new();
    let mut i = 0;
    while i < padded.len() {
        let mut win = vec![0.0f32; WINDOW_SAMPLES];
        let end = (i + WINDOW_SAMPLES).min(padded.len());
        win[..end - i].copy_from_slice(&padded[i..end]);
        let input = Tensor::from_array(([1usize, WINDOW_SAMPLES, 1usize], win)).map_err(|err| e("Entrada", &err))?;
        let outputs = session.run(ort::inputs![IN_NAME => input]).map_err(|err| e("Inferencia", &err))?;
        for (name, dst) in [(OUT_NOTE, &mut note_rows), (OUT_ONSET, &mut onset_rows)] {
            let (shape, data) = outputs[name].try_extract_tensor::<f32>().map_err(|err| e("Salida", &err))?;
            if shape.len() != 3 || shape[1] as usize != MODEL_FRAMES_PER_WINDOW || shape[2] as usize != N_PITCHES {
                return Err(format!("Forma de salida inesperada: {shape:?}"));
            }
            let a = cut * N_PITCHES;
            let b = (MODEL_FRAMES_PER_WINDOW - cut) * N_PITCHES;
            dst.extend_from_slice(&data[a..b]);
        }
        i += hop;
    }
    let n_frames = samples.len() / FFT_HOP;
    let mk = |mut d: Vec<f32>| {
        d.truncate(n_frames * N_PITCHES);
        let rows = d.len() / N_PITCHES;
        notes::Matrix { rows, data: d }
    };
    Ok((mk(note_rows), mk(onset_rows)))
}

/// Audio ya a 22 050 Hz mono → notas.
pub fn transcribe_samples(samples: &[f32], p: &Params) -> Result<Vec<NoteEvent>, String> {
    let (frames, onsets) = infer(samples)?;
    Ok(notes::to_events(&notes::frames_to_notes(&frames, &onsets, p)))
}

/// Archivo de audio (wav, flac, mp3, ogg, m4a…) → notas.
pub fn transcribe_file(path: &Path, p: &Params) -> Result<Vec<NoteEvent>, String> {
    let (mono, rate) = audio::decode_mono(path)?;
    let x = audio::resample(&mono, rate, SAMPLE_RATE as u32);
    transcribe_samples(&x, p)
}
