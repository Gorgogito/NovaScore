mod audio;
mod pdf;

use nova_core::io::{self, PedalEvent, Sound};
use nova_core::{Artic, Clef, Content, Document, Dynamic, Jump, Marker, PartUpdate, Pitch, Score, SpPos, Spanner, SpannerKind, TimeSig};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

struct AppState(Mutex<Document>);

type Res = Result<Score, String>;

fn lock<'a>(s: &'a State<AppState>) -> std::sync::MutexGuard<'a, Document> {
    s.0.lock().unwrap_or_else(|e| e.into_inner())
}

#[tauri::command]
fn get_score(state: State<AppState>) -> Score {
    lock(&state).score.clone()
}

#[tauri::command]
fn new_score(state: State<AppState>, title: String, num: u32, den: u32, fifths: i8, measures: usize) -> Result<Score, String> {
    if num == 0 || num > 32 || !matches!(den, 1 | 2 | 4 | 8 | 16) {
        return Err("invalid time signature".into());
    }
    let mut doc = lock(&state);
    doc.replace(Score::new(&title, TimeSig { num, den }, fifths.clamp(-7, 7), measures.clamp(1, 500)));
    Ok(doc.score.clone())
}

#[derive(Serialize)]
struct Entered {
    score: Score,
    measure: usize,
    start: u32,
}

fn check_pitch(step: u8, alter: i8) -> Result<(), String> {
    if step > 6 || !(-2..=2).contains(&alter) {
        return Err("invalid pitch".into());
    }
    Ok(())
}

/// Enter a note (notes crossing the barline are split and tied) and return the next cursor.
#[tauri::command]
fn enter_note(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32, ticks: u32, step: u8, alter: i8, octave: i8) -> Result<Entered, String> {
    check_pitch(step, alter)?;
    let mut doc = lock(&state);
    let (m, s) = doc
        .edit(|s| s.put_note(part, voice, measure, start, ticks, vec![Pitch { step, alter, octave }]))
        .map_err(|e| e.to_string())?;
    Ok(Entered { score: doc.score.clone(), measure: m, start: s })
}

#[tauri::command]
fn add_pitch(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32, step: u8, alter: i8, octave: i8) -> Res {
    check_pitch(step, alter)?;
    let mut doc = lock(&state);
    doc.edit(|s| s.add_pitch(part, voice, measure, start, Pitch { step, alter, octave })).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn toggle_tie(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.toggle_tie(part, voice, measure, start)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Set the dynamic marking that starts at an event; `None` (or the same marking) clears it.
#[tauri::command]
fn set_dynamic(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32, dynamic: Option<String>) -> Res {
    let d = match dynamic {
        Some(name) => Some(Dynamic::from_name(&name).ok_or("unknown dynamic")?),
        None => None,
    };
    let mut doc = lock(&state);
    doc.edit(|s| s.set_dynamic(part, voice, measure, start, d)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Toggle an articulation on the notes between two positions (inclusive).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn toggle_articulation(state: State<AppState>, part: usize, voice: usize, start_measure: usize, start_tick: u32, end_measure: usize, end_tick: u32, kind: String) -> Res {
    let a = Artic::from_name(&kind).ok_or("unknown articulation")?;
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.toggle_articulation(part, voice, SpPos { measure: start_measure, tick: start_tick }, SpPos { measure: end_measure, tick: end_tick }, a).map(|_| ())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Toggle the repeat sign at the start (`start`) or end of a measure.
#[tauri::command]
fn toggle_repeat(state: State<AppState>, measure: usize, start: bool) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.toggle_repeat(measure, start)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Add or remove a segno, coda, "To Coda" or fine marker on a measure.
#[tauri::command]
fn toggle_marker(state: State<AppState>, measure: usize, marker: Marker) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.toggle_marker(measure, marker)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Set the D.C./D.S. jump at the end of a measure (the same one again, or `None`, removes it).
#[tauri::command]
fn set_jump(state: State<AppState>, measure: usize, jump: Option<Jump>) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.set_jump(measure, jump)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Add a slur or hairpin between two positions, or remove it if it already exists.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn toggle_spanner(state: State<AppState>, part: usize, kind: String, voice: u8, start_measure: usize, start_tick: u32, end_measure: usize, end_tick: u32, endings: Option<u8>) -> Res {
    let kind = match kind.as_str() {
        "slur" => SpannerKind::Slur,
        "crescendo" => SpannerKind::Crescendo,
        "decrescendo" => SpannerKind::Decrescendo,
        "ottava8va" => SpannerKind::Ottava8va,
        "ottava8vb" => SpannerKind::Ottava8vb,
        "ottava15ma" => SpannerKind::Ottava15ma,
        "ottava15mb" => SpannerKind::Ottava15mb,
        "pedal" => SpannerKind::Pedal,
        "trill" => SpannerKind::Trill,
        "volta" => SpannerKind::Volta,
        _ => return Err("unknown spanner kind".into()),
    };
    let sp = Spanner { kind, voice, start: SpPos { measure: start_measure, tick: start_tick }, end: SpPos { measure: end_measure, tick: end_tick }, endings: endings.unwrap_or(0) };
    let mut doc = lock(&state);
    doc.edit(|s| s.toggle_spanner(part, sp).map(|_| ())).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Remove the slurs and hairpins of a voice that overlap a range.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn remove_spanners(state: State<AppState>, part: usize, voice: usize, start_measure: usize, start_tick: u32, end_measure: usize, end_tick: u32) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.remove_spanners_in(part, voice, SpPos { measure: start_measure, tick: start_tick }, SpPos { measure: end_measure, tick: end_tick }).map(|_| ())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn add_part(state: State<AppState>, name: String, clef: Clef, program: u8, percussion: bool) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.add_part(&name, clef, program, percussion);
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn remove_part(state: State<AppState>, part: usize) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.remove_part(part)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn update_part(state: State<AppState>, part: usize, update: PartUpdate) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.update_part(part, update)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn export_pdf(path: String, svgs: Vec<String>) -> Result<(), String> {
    let bytes = pdf::svgs_to_pdf(&svgs)?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// Path from `NOVASCORE_EXPORT_PDF`: when set, the app exports the open score to PDF and quits.
#[tauri::command]
fn startup_export() -> Option<String> {
    std::env::var("NOVASCORE_EXPORT_PDF").ok().filter(|s| !s.is_empty())
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

#[derive(Serialize)]
struct Playback {
    sounds: Vec<Sound>,
    /// MIDI channel of every part.
    channels: Vec<u8>,
    /// Sustain pedal presses and releases.
    pedals: Vec<PedalEvent>,
    /// Measures in the order they are played (repeats unfolded).
    order: Vec<usize>,
}

#[tauri::command]
fn get_sounds(state: State<AppState>) -> Playback {
    let doc = lock(&state);
    Playback { sounds: io::audible_sounds(&doc.score), channels: doc.score.midi_channels(), pedals: io::audible_pedals(&doc.score), order: doc.score.playback_order() }
}

/// Save as MuseScore 4: a `.mscz` archive, or a bare `.mscx` when the path ends in `.mscx`.
#[tauri::command]
fn export_mscz(state: State<AppState>, path: String) -> Result<(), String> {
    let doc = lock(&state);
    let lower = path.to_lowercase();
    if lower.ends_with(".mscx") {
        return std::fs::write(&path, nova_core::mscx_write::export_mscx(&doc.score)).map_err(|e| e.to_string());
    }
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let bytes = nova_core::mscx_write::export_mscz(&doc.score, &stem)?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

#[tauri::command]
fn export_midi(state: State<AppState>, path: String) -> Result<(), String> {
    std::fs::write(path, io::export_midi(&lock(&state).score)).map_err(|e| e.to_string())
}

#[tauri::command]
fn export_musicxml(state: State<AppState>, path: String) -> Result<(), String> {
    std::fs::write(path, io::export_musicxml(&lock(&state).score)).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Imported {
    score: Score,
    warnings: Vec<String>,
}

#[tauri::command]
fn import_mscz(state: State<AppState>, path: String) -> Result<Imported, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let r = nova_core::mscx::import_mscz(&bytes)?;
    let mut doc = lock(&state);
    doc.replace(r.score);
    Ok(Imported { score: doc.score.clone(), warnings: r.warnings })
}

#[tauri::command]
fn import_midi(state: State<AppState>, path: String) -> Result<Imported, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let r = io::import_midi(&bytes)?;
    let mut doc = lock(&state);
    doc.replace(r.score);
    Ok(Imported { score: doc.score.clone(), warnings: r.warnings })
}

/// Transcribe an audio file (Basic Pitch) into a new score. The heavy work runs on a worker
/// thread and outside the document lock so the window stays responsive.
/// Carpeta de modelos de IA descargados (dentro de los datos de la app).
fn models_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).join("modelos")
}

/// ¿Está el modelo de piano de alta precisión? Devuelve su ruta si existe.
#[tauri::command]
fn piano_model_status(app: tauri::AppHandle) -> Option<String> {
    let p = audio::piano_model_path(&models_dir(&app));
    p.is_file().then(|| p.display().to_string())
}

/// Bandera de cancelación de la descarga en curso.
static PIANO_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Descarga el modelo de piano (hilo aparte). Emite `piano-download` con `{done, total}` en bytes.
#[tauri::command]
async fn download_piano_model(app: tauri::AppHandle) -> Result<String, String> {
    let dest = audio::piano_model_path(&models_dir(&app));
    let handle = app.clone();
    let d = dest.clone();
    PIANO_CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        audio::download_verified(audio::PIANO_URL, &d, audio::PIANO_SHA256, audio::PIANO_SIZE, &PIANO_CANCEL, |done, total| {
            let _ = handle.emit("piano-download", serde_json::json!({ "done": done, "total": total }));
        })
    })
    .await
    .map_err(|e| format!("La descarga se interrumpió: {e}"))??;
    Ok(dest.display().to_string())
}

/// Cancela la descarga del modelo de piano en curso (no deja archivos a medias).
#[tauri::command]
fn cancel_piano_download() {
    PIANO_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Borra el modelo de piano instalado (libera 98 MB; se puede volver a descargar).
#[tauri::command]
fn delete_piano_model(app: tauri::AppHandle) -> Result<(), String> {
    audio::delete_piano_model(&models_dir(&app))
}

#[tauri::command]
async fn import_audio(app: tauri::AppHandle, state: State<'_, AppState>, path: String, options: audio::AudioImportOptions) -> Result<Imported, String> {
    let models = models_dir(&app);
    let r = tauri::async_runtime::spawn_blocking(move || audio::audio_to_score(&path, &options, &models))
        .await
        .map_err(|e| format!("La transcripción se interrumpió: {e}"))??;
    let mut doc = lock(&state);
    doc.replace(r.score);
    Ok(Imported { score: doc.score.clone(), warnings: r.warnings })
}

#[tauri::command]
fn import_musicxml(state: State<AppState>, path: String) -> Result<Imported, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let r = io::import_musicxml(&text)?;
    let mut doc = lock(&state);
    doc.replace(r.score);
    Ok(Imported { score: doc.score.clone(), warnings: r.warnings })
}

#[tauri::command]
fn enter_rest(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32, ticks: u32) -> Result<Entered, String> {
    let mut doc = lock(&state);
    let (m, s) = doc.edit(|s| s.put_rest(part, voice, measure, start, ticks)).map_err(|e| e.to_string())?;
    Ok(Entered { score: doc.score.clone(), measure: m, start: s })
}

/// Turn the event at `start` into a tuplet (e.g. 3:2 triplet).
#[tauri::command]
fn make_tuplet(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32, actual: u8, normal: u8) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.make_tuplet(part, voice, measure, start, actual, normal)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn clear_tuplet(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.clear_tuplet(part, voice, measure, start)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn delete_event(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.delete_event(part, voice, measure, start)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn transpose_event(state: State<AppState>, part: usize, voice: usize, measure: usize, start: u32, semitones: i32) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| s.transpose_event(part, voice, measure, start, semitones)).map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn add_measures(state: State<AppState>, count: usize) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.add_measures(count.clamp(1, 100));
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
/// Time signature from `measure` until the next change (measure 0 sets the initial one).
fn set_time(state: State<AppState>, measure: usize, num: u32, den: u32) -> Res {
    if num == 0 || num > 32 || !matches!(den, 1 | 2 | 4 | 8 | 16) {
        return Err("invalid time signature".into());
    }
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.set_time_at(measure, TimeSig { num, den });
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn set_fifths(state: State<AppState>, fifths: i8) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.fifths = fifths.clamp(-7, 7);
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn set_tempo(state: State<AppState>, tempo: u32) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.tempo = tempo.clamp(20, 300);
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Detect the chord symbols of the score from its notes (replaces the current ones).
#[tauri::command]
fn detect_chords(state: State<AppState>) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.detect_chord_symbols();
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

/// Fija (o con texto vacío borra) el acorde que empieza en `(measure, tick)`.
#[tauri::command]
fn set_chord(state: State<AppState>, measure: usize, tick: u32, text: String) -> Res {
    let mut doc = lock(&state);
    let mut err = None;
    doc.edit(|s| {
        err = s.set_chord(measure, tick, &text).err();
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    if let Some(e) = err {
        return Err(e);
    }
    Ok(doc.score.clone())
}

#[tauri::command]
fn clear_chords(state: State<AppState>) -> Res {
    let mut doc = lock(&state);
    doc.edit(|s| {
        s.chords.clear();
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(doc.score.clone())
}

#[tauri::command]
fn undo(state: State<AppState>) -> Score {
    let mut doc = lock(&state);
    doc.undo();
    doc.score.clone()
}

#[tauri::command]
fn redo(state: State<AppState>) -> Score {
    let mut doc = lock(&state);
    doc.redo();
    doc.score.clone()
}

#[tauri::command]
fn save_score(state: State<AppState>, path: String) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&lock(&state).score).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_score(state: State<AppState>, path: String) -> Res {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let score: Score = serde_json::from_str(&text).map_err(|e| format!("invalid .nova file: {e}"))?;
    let mut doc = lock(&state);
    doc.replace(score);
    Ok(doc.score.clone())
}

/// Score to show on start: the file given as first CLI argument (.nova / .musicxml / .xml), or a blank one.
fn initial_score() -> Score {
    if let Some(path) = std::env::args().nth(1) {
        let lower = path.to_lowercase();
        if audio::is_audio_path(&path) {
            match audio::audio_to_score(&path, &Default::default(), std::path::Path::new(".")) {
                Ok(r) => return r.score,
                Err(e) => eprintln!("No se pudo transcribir {path}: {e}"),
            }
        } else if lower.ends_with(".mscz") || lower.ends_with(".mscx") {
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(r) = nova_core::mscx::import_mscz(&bytes) {
                    return r.score;
                }
            }
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            if lower.ends_with(".nova") {
                if let Ok(s) = serde_json::from_str::<Score>(&text) {
                    return s;
                }
            } else if let Ok(r) = io::import_musicxml(&text) {
                return r.score;
            }
        }
    }
    Score::new("Sin título", TimeSig { num: 4, den: 4 }, 0, 16)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState(Mutex::new(Document::new(initial_score()))))
        .invoke_handler(tauri::generate_handler![
            get_score, toggle_repeat, toggle_marker, set_jump, set_dynamic, toggle_articulation, toggle_spanner, remove_spanners, add_part, remove_part, update_part, new_score, enter_note, add_pitch, toggle_tie, get_sounds, export_pdf, export_mscz, startup_export, quit, export_midi, export_musicxml, import_musicxml, import_midi, import_mscz, import_audio, piano_model_status, download_piano_model, cancel_piano_download, delete_piano_model, enter_rest, make_tuplet, clear_tuplet, delete_event, transpose_event, add_measures,
            set_time, set_fifths, set_tempo, detect_chords, set_chord, clear_chords, undo, redo, save_score, load_score
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
