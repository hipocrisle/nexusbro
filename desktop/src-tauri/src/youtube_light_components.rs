//! Component pins are sealed into this application at build time. The bundled
//! JSON is not a runtime authority; replacing it cannot authorize new binaries.
use crate::youtube_light_sidecar::Components;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};
include!(concat!(env!("OUT_DIR"), "/youtube-component-pins.rs"));

#[derive(Deserialize)]
struct Manifest {
    schema: u32,
    platform: String,
    components: BTreeMap<String, Record>,
}
#[derive(Deserialize)]
struct Record {
    filename: String,
    sha256: String,
}

fn verify(
    directory: &Path,
    record: &Record,
    expected: &str,
) -> Result<std::path::PathBuf, &'static str> {
    if record.filename != expected
        || record.sha256.len() != 64
        || !record
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Некорректные сведения о компонентах YouTube");
    }
    let path = directory.join(expected);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| "Компоненты YouTube недоступны")?;
    if !meta.file_type().is_file() || meta.len() == 0 || meta.len() > 256 * 1024 * 1024 {
        return Err("Компоненты YouTube недоступны");
    }
    let mut file = File::open(&path).map_err(|_| "Компоненты YouTube недоступны")?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| "Не удалось проверить компоненты YouTube")?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > 256 * 1024 * 1024 {
            return Err("Компоненты YouTube превышают лимит");
        }
        hash.update(&buffer[..read]);
    }
    if total != meta.len() || format!("{:x}", hash.finalize()) != record.sha256 {
        return Err("Проверка компонентов YouTube не пройдена");
    }
    Ok(path)
}

pub fn load(directory: &Path) -> Result<Components, &'static str> {
    if SEALED_YOUTUBE_MANIFEST.is_empty() {
        return Err("Компоненты YouTube не включены в сборку");
    }
    if !directory.is_absolute()
        || std::fs::symlink_metadata(directory).map_or(true, |m| !m.file_type().is_dir())
    {
        return Err("Компоненты YouTube недоступны");
    }
    let manifest: Manifest = serde_json::from_str(SEALED_YOUTUBE_MANIFEST)
        .map_err(|_| "Некорректные сведения о компонентах YouTube")?;
    let platform = if cfg!(target_os = "windows") {
        "windows-x64"
    } else {
        "linux-x64"
    };
    if manifest.schema != 1 || manifest.platform != platform {
        return Err("Компоненты YouTube другой платформы");
    }
    let extractor = manifest
        .components
        .get("yt-dlp")
        .ok_or("Нет компонента извлечения YouTube")?;
    let runtime = manifest
        .components
        .get("deno")
        .ok_or("Нет компонента обработки YouTube")?;
    let extractor = verify(
        directory,
        extractor,
        if cfg!(windows) {
            "yt-dlp.exe"
        } else {
            "yt-dlp"
        },
    )?;
    let runtime = verify(
        directory,
        runtime,
        if cfg!(windows) { "deno.exe" } else { "deno" },
    )?;
    Components::new(extractor, runtime).map_err(|_| "Компоненты YouTube недоступны")
}
