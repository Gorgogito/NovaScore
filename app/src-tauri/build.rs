use std::path::{Path, PathBuf};

/// Windows: copia `DirectML.dll` (la descarga `ort` al compilar) a `resources/` para que el
/// instalador la ponga junto al ejecutable; sin ella el motor de piano cae a CPU (3×–2× más lento).
fn stage_directml() {
    let dest = Path::new("resources");
    let _ = std::fs::create_dir_all(dest);
    let target = dest.join("DirectML.dll");
    if target.exists() {
        return;
    }
    let mut found: Option<PathBuf> = None;
    // 1) la copia que ort deja en target/<perfil>/ (OUT_DIR = target/<perfil>/build/<crate>/out)
    if let Ok(out) = std::env::var("OUT_DIR") {
        let p = Path::new(&out).ancestors().nth(3).map(|d| d.join("DirectML.dll"));
        found = p.filter(|p| p.exists());
    }
    // 2) la caché de ort (%LOCALAPPDATA%/ort.pyke.io/dfbin/<triple>/<hash>/DirectML.dll)
    if found.is_none() {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let root = Path::new(&local).join("ort.pyke.io").join("dfbin");
            if let Ok(triples) = std::fs::read_dir(&root) {
                'outer: for t in triples.flatten() {
                    if let Ok(hashes) = std::fs::read_dir(t.path()) {
                        for h in hashes.flatten() {
                            let c = h.path().join("DirectML.dll");
                            if c.exists() {
                                found = Some(c);
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }
    match found {
        Some(f) => {
            std::fs::copy(&f, &target).expect("copiar DirectML.dll");
        }
        None => println!("cargo:warning=DirectML.dll no encontrada: el instalador no la incluirá (el motor de piano usará la CPU)"),
    }
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        stage_directml();
    }
    tauri_build::build()
}
