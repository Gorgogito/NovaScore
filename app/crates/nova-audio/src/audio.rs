//! Decodificación (symphonia, MPL-2.0) y remuestreo a 22 050 Hz mono.

use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Audio mono (media de canales) y su frecuencia de muestreo original.
pub fn decode_mono(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("No se pudo abrir el audio: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Formato de audio no reconocido: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("El archivo no contiene pistas de audio")?;
    let id = track.id;
    let rate = track.codec_params.sample_rate.ok_or("Frecuencia de muestreo desconocida")?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Códec no soportado: {e}"))?;

    let mut out: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("Error leyendo el audio: {e}")),
        };
        if packet.track_id() != id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                let ch = spec.channels.count().max(1);
                let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                sb.copy_interleaved_ref(buf);
                for frame in sb.samples().chunks(ch) {
                    out.push(frame.iter().sum::<f32>() / ch as f32);
                }
            }
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Error decodificando: {e}")),
        }
    }
    if out.is_empty() {
        return Err("El audio está vacío".into());
    }
    Ok((out, rate))
}

/// Remuestreo por sinc con ventana de Hann (paso bajo si se reduce la frecuencia).
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return x.to_vec();
    }
    let ratio = from as f64 / to as f64; // muestras de entrada por muestra de salida
    let cutoff = (to as f64 / from as f64).min(1.0);
    let half = (16.0 / cutoff).ceil() as i64;
    let n_out = (x.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(n_out);
    for n in 0..n_out {
        let c = n as f64 * ratio;
        let base = c.floor() as i64;
        let mut acc = 0.0f64;
        for k in (base - half + 1)..=(base + half) {
            if k < 0 || k as usize >= x.len() {
                continue;
            }
            let t = k as f64 - c;
            let w = 0.5 * (1.0 + (std::f64::consts::PI * t / half as f64).cos());
            let a = std::f64::consts::PI * cutoff * t;
            let s = if a.abs() < 1e-12 { 1.0 } else { a.sin() / a };
            acc += x[k as usize] as f64 * cutoff * s * w;
        }
        out.push(acc as f32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_sine_frequency() {
        let sr = 44100.0;
        let x: Vec<f32> = (0..44100).map(|i| (2.0 * std::f64::consts::PI * 440.0 * i as f64 / sr).sin() as f32).collect();
        let y = resample(&x, 44100, 22050);
        assert_eq!(y.len(), 22050);
        let z = y.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((z as i32 - 440).abs() <= 1, "{z}");
        let m = y[2000..20000].iter().cloned().fold(0.0f32, f32::max);
        assert!((m - 1.0).abs() < 0.02, "{m}");
    }
}
