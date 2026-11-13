//! Reconocimiento óptico de partituras (PDF / imagen → partitura) con Audiveris como proceso aparte.
//!
//! Audiveris (AGPL-3, Java) no se incluye en el instalador: si el usuario lo tiene instalado se lanza en modo
//! por lotes y se importa el MusicXML que produce con los importadores de `nova-core`.

use nova_core::io::ImportReport;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Extensiones que Audiveris sabe leer.
pub const OMR_EXTENSIONS: [&str; 8] = ["pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif"];

/// Tiempo máximo de un reconocimiento.
const MAX_SECONDS: u64 = 20 * 60;

/// Ajustes guardados junto a los modelos de IA (`omr.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    audiveris: Option<String>,
}

/// Estado del motor para la interfaz.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmrStatus {
    pub found: bool,
    pub path: Option<String>,
    /// De dónde sale la ruta: «configurada», «variable AUDIVERIS» o «detectada».
    pub source: Option<String>,
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("omr.json")
}

fn load_settings(dir: &Path) -> Settings {
    std::fs::read_to_string(settings_path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn set_audiveris_path(dir: &Path, path: Option<String>) -> Result<(), String> {
    if let Some(p) = &path {
        if !Path::new(p).is_file() {
            return Err(format!("«{p}» no es un archivo."));
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&Settings { audiveris: path }).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(dir), text).map_err(|e| e.to_string())
}

/// Rutas habituales de instalación de Audiveris.
fn default_locations() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "LOCALAPPDATA", "ProgramW6432"] {
            if let Ok(base) = std::env::var(var) {
                v.push(Path::new(&base).join("Audiveris").join("Audiveris.exe"));
            }
        }
    } else if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Applications/Audiveris.app/Contents/MacOS/Audiveris"));
    } else {
        v.push(PathBuf::from("/opt/audiveris/bin/Audiveris"));
        v.push(PathBuf::from("/usr/bin/audiveris"));
        v.push(PathBuf::from("/usr/local/bin/audiveris"));
    }
    v
}

/// Busca un ejecutable en las carpetas del PATH.
fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names {
            let c = dir.join(n);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// Audiveris: ajuste del usuario, variable `AUDIVERIS`, PATH y rutas habituales (en ese orden).
pub fn find_audiveris(dir: &Path) -> Option<(PathBuf, &'static str)> {
    if let Some(p) = load_settings(dir).audiveris.map(PathBuf::from).filter(|p| p.is_file()) {
        return Some((p, "configurada"));
    }
    if let Some(p) = std::env::var_os("AUDIVERIS").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some((p, "variable AUDIVERIS"));
    }
    if let Some(p) = on_path(&["Audiveris.exe", "audiveris.exe", "Audiveris.bat", "audiveris.bat", "Audiveris", "audiveris"]) {
        return Some((p, "detectada en el PATH"));
    }
    default_locations().into_iter().find(|p| p.is_file()).map(|p| (p, "detectada"))
}

pub fn status(dir: &Path) -> OmrStatus {
    match find_audiveris(dir) {
        Some((p, source)) => OmrStatus { found: true, path: Some(p.to_string_lossy().into_owned()), source: Some(source.into()) },
        None => OmrStatus { found: false, path: None, source: None },
    }
}

/// Se pone a `true` para cancelar el reconocimiento en curso.
pub static CANCEL: AtomicBool = AtomicBool::new(false);
/// Un solo reconocimiento a la vez.
static RUNNING: Mutex<bool> = Mutex::new(false);

pub fn is_omr_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    OMR_EXTENSIONS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// MusicXML producidos por Audiveris en `dir` (busca en las subcarpetas), el primer movimiento primero.
fn find_outputs(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
                if matches!(ext.to_lowercase().as_str(), "mxl" | "musicxml") || (ext.eq_ignore_ascii_case("xml") && !p.to_string_lossy().to_lowercase().contains("log")) {
                    out.push(p);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort_by_key(|p| natural_key(&p.to_string_lossy()));
    out
}

/// Clave de orden «natural»: `mvt2` va antes que `mvt10`.
fn natural_key(s: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let (mut text, mut num, mut in_num) = (String::new(), String::new(), false);
    for c in s.to_lowercase().chars() {
        if c.is_ascii_digit() {
            if !in_num {
                out.push((std::mem::take(&mut text), 0));
                in_num = true;
            }
            num.push(c);
        } else {
            if in_num {
                let n = num.parse::<u64>().unwrap_or(0);
                if let Some(last) = out.last_mut() {
                    last.1 = n;
                }
                num.clear();
                in_num = false;
            }
            text.push(c);
        }
    }
    if in_num {
        if let Some(last) = out.last_mut() {
            last.1 = num.parse::<u64>().unwrap_or(0);
        }
    }
    out.push((text, 0));
    out
}

fn no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Carpeta con los datos de idioma del OCR (`*.traineddata`) para pasársela a Audiveris como `TESSDATA_PREFIX`:
/// la de NovaScore (`<ajustes>/tessdata`) o la que haya junto a Audiveris. `None` si ya está definida la variable o no hay ninguna.
fn tessdata_dir(settings_dir: &Path, exe: &Path) -> Option<PathBuf> {
    if std::env::var_os("TESSDATA_PREFIX").is_some() {
        return None;
    }
    let mut cands = vec![settings_dir.join("tessdata")];
    for up in exe.ancestors().skip(1).take(3) {
        cands.push(up.join("tessdata"));
    }
    cands.into_iter().find(|d| {
        std::fs::read_dir(d).map_or(false, |rd| rd.flatten().any(|f| f.path().extension().map_or(false, |e| e == "traineddata")))
    })
}

/// Aviso si el registro de Audiveris dice que el OCR no pudo cargar ningún idioma (sin él no lee letra, títulos ni textos).
fn ocr_problem(out_dir: &Path) -> Option<String> {
    fn logs(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                logs(&p, out);
            } else if p.extension().map_or(false, |x| x == "log") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    logs(out_dir, &mut files);
    let bad = files.iter().any(|f| {
        std::fs::read_to_string(f).map_or(false, |t| t.contains("No installed OCR languages") || t.contains("Could not initialize TessBaseAPI") || t.contains("Missing support for"))
    });
    bad.then(|| "El OCR de Audiveris no tiene idiomas instalados (o no se pudieron cargar): no se han leído la letra, los títulos ni los textos. Instala los datos de idioma completos de Tesseract (eng.traineddata, spa.traineddata del repositorio «tessdata») en una carpeta «tessdata» junto a Audiveris o en la carpeta de ajustes de NovaScore, o define TESSDATA_PREFIX.".to_string())
}

/// Reconoce una partitura impresa: lanza Audiveris sobre `input` y devuelve la partitura importada.
pub fn recognize(settings_dir: &Path, input: &str, exe_override: Option<&Path>) -> Result<ImportReport, String> {
    if !is_omr_path(input) {
        return Err("Formato no admitido: usa un PDF o una imagen (png, jpg, tif, bmp).".into());
    }
    if !Path::new(input).is_file() {
        return Err(format!("No se encuentra el archivo {input}."));
    }
    let exe = match exe_override {
        Some(p) => p.to_path_buf(),
        None => find_audiveris(settings_dir).map(|f| f.0).ok_or("Audiveris no está instalado o no se ha encontrado.")?,
    };
    {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if *running {
            return Err("Ya hay un reconocimiento en curso.".into());
        }
        *running = true;
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = false;
        }
    }
    let _done = Done;
    CANCEL.store(false, Ordering::SeqCst);

    let out_dir = std::env::temp_dir().join(format!("novascore-omr-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    // the engine may still hold files for a moment after it is stopped: try again a few times
    let cleanup = |d: &Path| {
        for _ in 0..12 {
            if std::fs::remove_dir_all(d).is_ok() || !d.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    };
    // Audiveris.exe launches a Java process: stopping the launcher alone can leave Java running, so the whole tree goes
    let stop = |child: &mut std::process::Child| {
        #[cfg(windows)]
        {
            let mut k = std::process::Command::new("taskkill");
            k.args(["/PID", &child.id().to_string(), "/T", "/F"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            no_window(&mut k);
            let _ = k.status();
        }
        let _ = child.kill();
        let _ = child.wait();
    };

    let mut cmd = if exe.extension().map_or(false, |e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd")) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(&exe);
        c
    } else {
        std::process::Command::new(&exe)
    };
    // a scanned or photographed page that is tilted is straightened first (see deskew.rs)
    let (engine_input, tilt) = match crate::deskew::deskew_file(Path::new(input), &out_dir) {
        Some((p, a)) => (p, Some(a)),
        None => (PathBuf::from(input), None),
    };
    cmd.args(["-batch", "-export", "-output"]).arg(&out_dir).arg(&engine_input).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    let log_path = out_dir.join("audiveris-stderr.log");
    if let Ok(f) = std::fs::File::create(&log_path) {
        cmd.stderr(f);
    }
    if let Some(d) = tessdata_dir(settings_dir, &exe) {
        cmd.env("TESSDATA_PREFIX", d);
    }
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| {
        cleanup(&out_dir);
        format!("No se pudo lanzar Audiveris ({}): {e}", exe.display())
    })?;
    let started = Instant::now();
    let status = loop {
        if CANCEL.load(Ordering::SeqCst) {
            stop(&mut child);
            cleanup(&out_dir);
            return Err("Reconocimiento cancelado.".into());
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                cleanup(&out_dir);
                return Err(format!("Error esperando a Audiveris: {e}"));
            }
        }
        if started.elapsed() > Duration::from_secs(MAX_SECONDS) {
            stop(&mut child);
            cleanup(&out_dir);
            return Err(format!("Audiveris tardó más de {} minutos y se detuvo.", MAX_SECONDS / 60));
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    let outputs = find_outputs(&out_dir);
    let result = (|| {
        let first = outputs.first().ok_or_else(|| {
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            let tail: String = log.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
            format!(
                "Audiveris no produjo ninguna partitura{}{}. Comprueba que el archivo sea una partitura legible (mejor un PDF o una imagen de 300 ppp).",
                if status.success() { "" } else { " (terminó con error)" },
                if tail.trim().is_empty() { String::new() } else { format!(": {}", tail.trim()) }
            )
        })?;
        let bytes = std::fs::read(first).map_err(|e| e.to_string())?;
        let mut r = nova_core::io::import_musicxml_bytes(&bytes)?;
        // several movements: one after the other, in the order Audiveris numbers them
        let (mut joined, mut key_differs, mut unreadable) = (1usize, false, 0usize);
        for extra in &outputs[1..] {
            match std::fs::read(extra).map_err(|e| e.to_string()).and_then(|b| nova_core::io::import_musicxml_bytes(&b)) {
                Ok(m) => {
                    key_differs |= r.score.append(&m.score);
                    joined += 1;
                }
                Err(_) => unreadable += 1,
            }
        }
        let name = Path::new(input).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if r.score.title.trim().is_empty() || r.score.title == "Sin título" || r.score.title == "MIDI importado" {
            r.score.title = name;
        }
        r.warnings.insert(0, "Reconocimiento óptico (Audiveris): es un borrador, revisa notas, ritmos, alteraciones y letra frente a la partitura original.".into());
        if let Some(a) = tilt {
            r.warnings.push(format!("La imagen estaba girada {a:.1}° y se enderezó antes de reconocerla."));
        }
        if let Some(w) = ocr_problem(&out_dir) {
            r.warnings.push(w);
        }
        if joined > 1 {
            r.warnings.push(format!("Audiveris separó la obra en {joined} movimientos: se unieron uno tras otro (los cambios de compás se conservan)."));
        }
        if key_differs {
            r.warnings.push("Algún movimiento tiene otra tonalidad: se ha añadido un cambio de armadura donde empieza; revisa que sea el que esperabas.".into());
        }
        if unreadable > 0 {
            r.warnings.push(format!("{unreadable} movimiento(s) no se pudieron leer y se omitieron."));
        }
        Ok(r)
    })();
    cleanup(&out_dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("novascore-omr-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn natural_order_puts_movement_2_before_10() {
        let mut v = vec!["a/obra.mvt10.mxl", "a/obra.mvt2.mxl", "a/obra.mvt1.mxl"];
        v.sort_by_key(|p| natural_key(p));
        assert_eq!(v, vec!["a/obra.mvt1.mxl", "a/obra.mvt2.mxl", "a/obra.mvt10.mxl"]);
    }

    #[test]
    fn recognizes_image_and_pdf_extensions() {
        assert!(is_omr_path("C:\\x\\Partitura.PDF") && is_omr_path("a.png") && is_omr_path("a.JPEG") && is_omr_path("a.tif"));
        assert!(!is_omr_path("a.wav") && !is_omr_path("a.musicxml") && !is_omr_path("pdf"));
    }

    #[test]
    fn settings_are_saved_and_checked() {
        let d = scratch("settings");
        assert!(set_audiveris_path(&d, Some(d.join("no-existe.exe").to_string_lossy().into_owned())).is_err());
        let exe = d.join("fake.exe");
        std::fs::write(&exe, b"x").unwrap();
        set_audiveris_path(&d, Some(exe.to_string_lossy().into_owned())).unwrap();
        let (p, src) = find_audiveris(&d).unwrap();
        assert_eq!((p, src), (exe, "configurada"));
        set_audiveris_path(&d, None).unwrap();
        assert!(load_settings(&d).audiveris.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un «Audiveris» de mentira: copia una partitura de ejemplo a `<salida>/<nombre>/<nombre>.xml`, como haría el real.
    fn fake_audiveris(dir: &Path, movements: u32) -> PathBuf {
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("samples");
        let plain = |p: PathBuf| p.canonicalize().unwrap().display().to_string().trim_start_matches(r"\\?\").to_string();
        let (first, second) = (plain(samples.join("demo.musicxml")), plain(samples.join("tuplets.musicxml")));
        // The engine receives `-batch -export -output <dir> <input>`: the output folder is argument 4 and the input 5.
        // It writes `<dir>/<name>/<name>.xml` (one movement) or `.mvt1.xml` and `.mvt2.xml` (two).
        let (exe, body) = if cfg!(windows) {
            let body = match movements {
                0 => "@echo off\r\necho no se pudo leer la imagen 1>&2\r\nexit /b 1\r\n".to_string(),
                1 => format!("@echo off\r\nset OUT=%~4\r\nmkdir \"%OUT%\\%~n5\"\r\ncopy /Y \"{first}\" \"%OUT%\\%~n5\\%~n5.xml\" >nul\r\nexit /b 0\r\n"),
                _ => format!("@echo off\r\nset OUT=%~4\r\nmkdir \"%OUT%\\%~n5\"\r\ncopy /Y \"{first}\" \"%OUT%\\%~n5\\%~n5.mvt1.xml\" >nul\r\ncopy /Y \"{second}\" \"%OUT%\\%~n5\\%~n5.mvt2.xml\" >nul\r\nexit /b 0\r\n"),
            };
            (dir.join("fake-audiveris.cmd"), body)
        } else {
            let head = "#!/bin/sh\nout=\"$4\"\nname=$(basename \"$5\")\nname=\"${name%.*}\"\nmkdir -p \"$out/$name\"\n";
            let body = match movements {
                0 => "#!/bin/sh\necho 'no se pudo leer la imagen' >&2\nexit 1\n".to_string(),
                1 => format!("{head}cp \"{first}\" \"$out/$name/$name.xml\"\n"),
                _ => format!("{head}cp \"{first}\" \"$out/$name/$name.mvt1.xml\"\ncp \"{second}\" \"$out/$name/$name.mvt2.xml\"\n"),
            };
            (dir.join("fake-audiveris.sh"), body)
        };
        std::fs::write(&exe, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        exe
    }

    #[test]
    fn imports_what_the_engine_writes_and_reports_failures() {
        let d = scratch("run");
        let input = d.join("Mi partitura.pdf");
        std::fs::write(&input, b"%PDF-1.4").unwrap();
        // the tests of this module share the single-run lock: run them one after the other
        let ok = fake_audiveris(&d, 1);
        let r = recognize(&d, &input.to_string_lossy(), Some(&ok)).unwrap();
        assert!(!r.score.parts.is_empty() && r.score.parts[0].measures.iter().any(|m| !m.events.is_empty()));
        assert!(r.warnings[0].contains("Audiveris"), "{:?}", r.warnings);
        let bad = fake_audiveris(&d, 0);
        let e = recognize(&d, &input.to_string_lossy(), Some(&bad)).unwrap_err();
        assert!(e.contains("no produjo ninguna partitura") && e.contains("no se pudo leer la imagen"), "{e}");
        // two movements are joined in order, with a time change where the signature differs
        let two = fake_audiveris(&d, 2);
        let joined = recognize(&d, &input.to_string_lossy(), Some(&two)).unwrap();
        let (a, b) = (nova_core::io::import_musicxml(include_str!("../../../samples/demo.musicxml")).unwrap().score, nova_core::io::import_musicxml(include_str!("../../../samples/tuplets.musicxml")).unwrap().score);
        assert_eq!(joined.score.parts[0].measures.len(), a.parts[0].measures.len() + b.parts[0].measures.len(), "{:?}", joined.warnings);
        assert!(joined.warnings.iter().any(|w| w.contains("2 movimientos")), "{:?}", joined.warnings);
        // a slow engine can be cancelled and leaves nothing behind
        let slow = d.join(if cfg!(windows) { "slow-audiveris.cmd" } else { "slow-audiveris.sh" });
        std::fs::write(&slow, if cfg!(windows) { "@echo off
ping -n 30 127.0.0.1 >nul
" } else { "#!/bin/sh
sleep 30
" }).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let canceller = std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            CANCEL.store(true, Ordering::SeqCst);
        });
        let t = Instant::now();
        let e = recognize(&d, &input.to_string_lossy(), Some(&slow)).unwrap_err();
        canceller.join().unwrap();
        assert!(e.contains("cancelado") && t.elapsed() < Duration::from_secs(15), "{e} after {:?}", t.elapsed());
        // wrong format and missing file never reach the engine
        assert!(recognize(&d, &d.join("x.wav").to_string_lossy(), Some(&ok)).unwrap_err().contains("Formato"));
        assert!(recognize(&d, &d.join("no.pdf").to_string_lossy(), Some(&ok)).unwrap_err().contains("No se encuentra"));
        // the temporary output is removed
        assert!(std::fs::read_dir(std::env::temp_dir()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(&format!("novascore-omr-{}-", std::process::id()))));
        let _ = std::fs::remove_dir_all(&d);
    }
}
