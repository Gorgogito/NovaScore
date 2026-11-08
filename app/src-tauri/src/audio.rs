//! Audio → partitura: une `nova-audio` (Basic Pitch) con `nova_core::quantize`.

use nova_core::io::ImportReport;
use nova_core::quantize::{notes_to_score, QuantizeOptions, RawNote};
use nova_core::TimeSig;
use serde::Deserialize;

/// Extensiones de audio que la app sabe abrir.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["wav", "mp3", "flac", "ogg", "oga", "m4a", "aac"];

pub fn is_audio_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    AUDIO_EXTENSIONS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// Opciones que envía la interfaz (todas opcionales: sin ellas se usa lo automático).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioImportOptions {
    /// Amplitud mínima (0..1) para conservar una nota.
    pub min_amplitude: Option<f32>,
    /// Umbral relativo a las notas fuertes (0..1); tiene prioridad sobre `min_amplitude`.
    pub min_relative: Option<f32>,
    /// Negras por minuto; `None` = estimar.
    pub tempo: Option<f64>,
    pub num: Option<u32>,
    pub den: Option<u32>,
    /// Voces por pentagrama (1 = más limpio, 2 = separa notas largas de la melodía).
    pub max_voices: Option<usize>,
}

pub fn audio_to_score(path: &str, o: &AudioImportOptions) -> Result<ImportReport, String> {
    let notes = nova_audio::transcribe_file(std::path::Path::new(path), &nova_audio::Params::default())?;
    let raw: Vec<RawNote> = notes
        .iter()
        .map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset })
        .collect();
    let mut q = QuantizeOptions::default();
    if let Some(a) = o.min_amplitude {
        q.min_amplitude = a.clamp(0.0, 1.0);
        q.min_relative = None;
    }
    if let Some(r) = o.min_relative {
        q.min_relative = Some(r.clamp(0.1, 1.0));
    }
    q.tempo = o.tempo.filter(|t| *t > 0.0);
    if let Some(v) = o.max_voices {
        q.max_voices = v.clamp(1, 4);
    }
    if let (Some(num), Some(den)) = (o.num, o.den) {
        if (1..=32).contains(&num) && matches!(den, 1 | 2 | 4 | 8 | 16) {
            q.time = TimeSig { num, den };
        }
    }
    if let Some(stem) = std::path::Path::new(path).file_stem().and_then(|s| s.to_str()) {
        q.title = stem.to_string();
    }
    let mut r = notes_to_score(&raw, &q)?;
    r.warnings.insert(0, format!("Transcripción automática ({} notas detectadas): es un borrador, revísalo.", raw.len()));
    Ok(r)
}
