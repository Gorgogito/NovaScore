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
    /// «general» (Basic Pitch, incluido) o «piano» (modelo de alta precisión, descargable).
    pub engine: Option<String>,
}

/// Nombre del archivo del modelo de piano dentro de la carpeta de modelos.
pub const PIANO_MODEL_FILE: &str = "piano.onnx";

/// Dónde se publica el modelo de piano (release `models-v1`, ver `docs/MODELOS.md`).
pub const PIANO_URL: &str = "https://github.com/Gorgogito/NovaScore/releases/download/models-v1/piano.onnx";
pub const PIANO_SHA256: &str = "2b43528170ada21dc95da555b39f9926239ebd5caec061daac8614d36eb59546";
pub const PIANO_SIZE: u64 = 98_684_797;

/// Descarga `url` a `dest` comprobando tamaño y SHA-256; escribe en `dest.part` y renombra al final,
/// así nunca queda un modelo a medias. `progress(descargado, total)` se llama cada ~256 KB.
pub fn download_verified(url: &str, dest: &std::path::Path, sha256: &str, size: u64, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear {}: {e}", dir.display()))?;
    }
    let part = dest.with_extension("part");
    let resp = ureq::get(url).call().map_err(|e| format!("No se pudo descargar el modelo: {e}"))?;
    let total = resp.body().content_length().unwrap_or(size);
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| format!("No se pudo escribir {}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let (mut done, mut last) = (0u64, 0u64);
    let result = (|| -> Result<(), String> {
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("Descarga interrumpida: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| format!("No se pudo escribir el modelo: {e}"))?;
            done += n as u64;
            if done - last >= 256 * 1024 {
                last = done;
                progress(done, total);
            }
        }
        progress(done, total);
        if done != size {
            return Err(format!("El modelo descargado tiene {done} bytes y se esperaban {size}."));
        }
        let got: String = hasher.finalize_reset().iter().map(|b| format!("{b:02x}")).collect();
        if got != sha256 {
            return Err("La descarga está corrupta (la suma SHA-256 no coincide).".into());
        }
        Ok(())
    })();
    drop(file);
    match result {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| format!("No se pudo instalar el modelo: {e}")),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Dónde está el modelo de piano: `NOVASCORE_PIANO_MODEL` o `<datos de la app>/modelos/piano.onnx`.
pub fn piano_model_path(models_dir: &std::path::Path) -> std::path::PathBuf {
    match std::env::var_os("NOVASCORE_PIANO_MODEL") {
        Some(p) => p.into(),
        None => models_dir.join(PIANO_MODEL_FILE),
    }
}

pub fn audio_to_score(path: &str, o: &AudioImportOptions, models_dir: &std::path::Path) -> Result<ImportReport, String> {
    let piano = o.engine.as_deref() == Some("piano");
    let notes = if piano {
        let model = piano_model_path(models_dir);
        if !model.is_file() {
            return Err(format!("Falta el modelo de piano ({}). Descárgalo o elige el motor general.", model.display()));
        }
        nova_audio::piano::transcribe_file(&model, nova_audio::piano::Device::Auto, std::path::Path::new(path), &Default::default())?
    } else {
        nova_audio::transcribe_file(std::path::Path::new(path), &nova_audio::Params::default())?
    };
    let raw: Vec<RawNote> = notes
        .iter()
        .map(|n| RawNote { start: n.start, end: n.end, pitch: n.pitch, amplitude: n.amplitude, onset: n.onset })
        .collect();
    let mut q = QuantizeOptions::default();
    if piano {
        // el modelo de piano ya es preciso: la velocidad baja es una nota suave real, no ruido
        q.min_relative = None;
        q.min_amplitude = 0.0;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Descarga real del modelo (98 MB); se ejecuta a mano: `cargo test -- --ignored download`.
    #[test]
    #[ignore]
    fn download_real_model() {
        let dest = std::env::temp_dir().join("novascore-test-models").join(PIANO_MODEL_FILE);
        let _ = std::fs::remove_file(&dest);
        let mut last = 0;
        download_verified(PIANO_URL, &dest, PIANO_SHA256, PIANO_SIZE, |d, t| {
            last = d;
            assert!(d <= t);
        })
        .unwrap();
        assert_eq!(last, PIANO_SIZE);
        assert!(dest.is_file() && !dest.with_extension("part").exists());
    }

    #[test]
    fn download_rejects_bad_url_and_leaves_no_file() {
        let dest = std::env::temp_dir().join("novascore-test-models").join("x.onnx");
        let _ = std::fs::remove_file(&dest);
        assert!(download_verified("http://127.0.0.1:1/nada", &dest, "00", 1, |_, _| {}).is_err());
        assert!(!dest.exists() && !dest.with_extension("part").exists());
    }

    /// WAV mono de 16 bits con una melodía de tonos puros: (inicio s, duración s, MIDI).
    fn write_wav(path: &std::path::Path, notes: &[(f64, f64, i32)], secs: f64) {
        let sr = 22050u32;
        let n = (secs * sr as f64) as usize;
        let mut x = vec![0.0f64; n];
        for &(s, d, m) in notes {
            let f = 440.0 * 2f64.powf((m as f64 - 69.0) / 12.0);
            for i in 0..(d * sr as f64) as usize {
                let k = (s * sr as f64) as usize + i;
                if k < n {
                    let t = i as f64 / sr as f64;
                    let env = (t / 0.01).min(1.0) * (-t).exp() * ((d - t) / 0.04).clamp(0.0, 1.0);
                    x[k] += 0.25 * env * (std::f64::consts::TAU * f * t).sin();
                }
            }
        }
        let data: Vec<u8> = x.iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&sr.to_le_bytes());
        out.extend_from_slice(&(sr * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn options_parse_from_the_json_the_ui_sends() {
        // lo que envía el diálogo (App.tsx): claves en camelCase; `tempo` ausente = automático
        let o: AudioImportOptions = serde_json::from_str(r#"{"minRelative":0.65,"num":3,"den":4,"maxVoices":2}"#).unwrap();
        assert_eq!((o.min_relative, o.num, o.den, o.max_voices, o.tempo), (Some(0.65), Some(3), Some(4), Some(2), None));
        let o: AudioImportOptions = serde_json::from_str(r#"{"tempo":84}"#).unwrap();
        assert_eq!(o.tempo, Some(84.0));
        let o: AudioImportOptions = serde_json::from_str("{}").unwrap();
        assert!(o.min_relative.is_none() && o.max_voices.is_none());
    }

    #[test]
    fn audio_file_becomes_a_score_honouring_the_options() {
        let dir = std::env::temp_dir().join("novascore_audio_test");
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("escala.wav");
        // do re mi fa sol fa mi re, una nota por segundo (negra = 1 s a 60 por minuto)
        let notes: Vec<(f64, f64, i32)> = [60, 62, 64, 65, 67, 65, 64, 62].iter().enumerate().map(|(i, m)| (0.5 + i as f64, 0.95, *m)).collect();
        write_wav(&wav, &notes, 9.5);
        let o = AudioImportOptions { tempo: Some(60.0), num: Some(4), den: Some(4), max_voices: Some(1), ..Default::default() };
        let r = audio_to_score(wav.to_str().unwrap(), &o, std::path::Path::new(".")).unwrap();
        assert_eq!(r.score.tempo, 60);
        assert_eq!((r.score.time.num, r.score.time.den), (4, 4));
        assert_eq!(r.score.title, "escala");
        let n = nova_core::io::sounds(&r.score).len();
        assert!((6..=10).contains(&n), "{n} notas");
        assert!(r.warnings[0].contains("borrador"));
        // un archivo que no existe da un error legible, no un pánico
        assert!(audio_to_score(dir.join("no_existe.wav").to_str().unwrap(), &Default::default(), std::path::Path::new(".")).is_err());
    }
}
