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

/// Local muxer authority comes only from compiled pins, never runtime JSON.
/// Load and recheck on a background worker; hashing is blocking IO.
pub struct MuxComponents {
    directory: std::path::PathBuf,
    ffmpeg: std::path::PathBuf,
    ffprobe: Option<std::path::PathBuf>,
    files: Vec<MuxFile>,
}
impl std::fmt::Debug for MuxComponents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MuxComponents([verified local files])")
    }
}
#[derive(Clone, Deserialize)]
struct MuxFile {
    filename: String,
    sha256: String,
    bytes: u64,
}
#[derive(Deserialize)]
struct MuxRecord {
    filename: String,
    sha256: String,
    #[serde(default)]
    ffprobe_filename: Option<String>,
    files: Vec<MuxFile>,
}
#[derive(Deserialize)]
struct MuxManifest {
    schema: u32,
    platform: String,
    components: BTreeMap<String, serde_json::Value>,
}
fn closed_path(directory: &Path, relative: &str) -> Result<std::path::PathBuf, &'static str> {
    let relative_path = Path::new(relative);
    if relative.len() > 512
        || relative.contains('\\')
        || relative.contains(':')
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Некорректный путь компонента обработки медиа");
    }
    let mut path = directory.to_path_buf();
    let parts = relative_path.components().collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        path.push(part.as_os_str());
        let meta =
            std::fs::symlink_metadata(&path).map_err(|_| "Компонент обработки медиа недоступен")?;
        if meta.file_type().is_symlink() || (index + 1 < parts.len() && !meta.is_dir()) {
            return Err("Небезопасный путь компонента обработки медиа");
        }
    }
    Ok(path)
}
impl MuxComponents {
    pub(crate) fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }
    pub(crate) fn ffprobe(&self) -> Option<&Path> {
        self.ffprobe.as_deref()
    }
    pub(crate) fn bin(&self) -> std::path::PathBuf {
        self.directory.join("ffmpeg/bin")
    }
    pub(crate) fn lib(&self) -> std::path::PathBuf {
        self.directory.join("ffmpeg/lib")
    }
    pub(crate) fn recheck(&self) -> Result<(), &'static str> {
        let mut aggregate = 0u64;
        for record in &self.files {
            let _ = closed_path(&self.directory, &record.filename)?;
            aggregate = aggregate
                .checked_add(record.bytes)
                .ok_or("Размер компонентов обработки медиа превышен")?;
            if aggregate > 512 * 1024 * 1024 {
                return Err("Размер компонентов обработки медиа превышен");
            }
            let verified = verify(
                &self.directory,
                &Record {
                    filename: record.filename.clone(),
                    sha256: record.sha256.clone(),
                },
                &record.filename,
            )?;
            if std::fs::metadata(verified)
                .map_err(|_| "Компонент обработки медиа недоступен")?
                .len()
                != record.bytes
            {
                return Err("Размер компонента обработки медиа изменился");
            }
        }
        // Reject extra files in the only dynamic loader search roots. No PATH
        // fallback or unsealed DLL/SO may accompany a correctly hashed FFmpeg.
        for relative in ["ffmpeg/bin", "ffmpeg/lib"] {
            let root = self.directory.join(relative);
            if !root.exists() {
                if relative.ends_with("/lib") && cfg!(windows) {
                    continue;
                }
                return Err("Нет каталога библиотек обработки медиа");
            }
            if !std::fs::symlink_metadata(&root)
                .map_err(|_| "Каталог библиотек недоступен")?
                .is_dir()
            {
                return Err("Небезопасный каталог библиотек");
            }
            for (index, entry) in std::fs::read_dir(root)
                .map_err(|_| "Каталог библиотек недоступен")?
                .enumerate()
            {
                if index >= 256 {
                    return Err("Слишком много библиотек обработки медиа");
                }
                let entry = entry.map_err(|_| "Каталог библиотек недоступен")?;
                let filename = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "Некорректное имя библиотеки")?;
                let expected = format!("{relative}/{filename}");
                if !self.files.iter().any(|f| f.filename == expected) {
                    return Err("Непроверенная библиотека обработки медиа");
                }
            }
        }
        Ok(())
    }
}
/// Existing extractor load() remains unchanged. All mux files are verified from
/// the sealed build-time manifest; ffprobe is optional only if not listed.
pub fn load_mux(directory: &Path) -> Result<MuxComponents, &'static str> {
    if SEALED_YOUTUBE_MANIFEST.is_empty()
        || !directory.is_absolute()
        || std::fs::symlink_metadata(directory)
            .map_or(true, |m| !m.is_dir() || m.file_type().is_symlink())
    {
        return Err("Компоненты обработки медиа недоступны");
    }
    let manifest: MuxManifest = serde_json::from_str(SEALED_YOUTUBE_MANIFEST)
        .map_err(|_| "Некорректные сведения о компонентах")?;
    if manifest.schema != 1
        || manifest.platform
            != if cfg!(windows) {
                "windows-x64"
            } else {
                "linux-x64"
            }
    {
        return Err("Компоненты обработки медиа другой платформы");
    }
    let record: MuxRecord = serde_json::from_value(
        manifest
            .components
            .get("ffmpeg")
            .ok_or("Компонент обработки медиа не включён")?
            .clone(),
    )
    .map_err(|_| "Некорректные сведения об обработке медиа")?;
    if record.files.is_empty() || record.files.len() > 256 {
        return Err("Некорректный набор файлов обработки медиа");
    }
    let ffmpeg_name = if cfg!(windows) {
        "ffmpeg/bin/ffmpeg.exe"
    } else {
        "ffmpeg/bin/ffmpeg"
    };
    let probe_name = if cfg!(windows) {
        "ffmpeg/bin/ffprobe.exe"
    } else {
        "ffmpeg/bin/ffprobe"
    };
    if record.filename != ffmpeg_name
        || !record
            .files
            .iter()
            .any(|f| f.filename == record.filename && f.sha256 == record.sha256)
    {
        return Err("Не проверен обработчик медиа");
    }
    let mut names = std::collections::BTreeSet::new();
    for file in &record.files {
        if !file.filename.starts_with("ffmpeg/")
            || !names.insert(file.filename.to_ascii_lowercase())
            || file.bytes == 0
        {
            return Err("Некорректный набор файлов обработки медиа");
        }
    }
    let ffmpeg = closed_path(directory, ffmpeg_name)?;
    let ffprobe = match record.ffprobe_filename {
        Some(name) if name == probe_name && record.files.iter().any(|f| f.filename == name) => {
            Some(closed_path(directory, &name)?)
        }
        None => None,
        _ => return Err("Не проверен анализатор медиа"),
    };
    let components = MuxComponents {
        directory: directory.to_path_buf(),
        ffmpeg,
        ffprobe,
        files: record.files,
    };
    components.recheck()?;
    Ok(components)
}

#[cfg(test)]
mod mux_tests {
    use super::*;
    use rand_core::RngCore;
    #[test]
    fn mux_closure_hashes_and_extra_loader_files_fail_closed() {
        let root = std::env::temp_dir().join(format!(
            "nb-mux-pins-{}-{}",
            std::process::id(),
            rand_core::OsRng.next_u64()
        ));
        std::fs::create_dir_all(root.join("ffmpeg/bin")).unwrap();
        std::fs::create_dir_all(root.join("ffmpeg/lib")).unwrap();
        let exe = if cfg!(windows) {
            "ffmpeg/bin/ffmpeg.exe"
        } else {
            "ffmpeg/bin/ffmpeg"
        };
        let library = if cfg!(windows) {
            "ffmpeg/bin/avcodec.dll"
        } else {
            "ffmpeg/lib/libavcodec.so.1"
        };
        let mut files = Vec::new();
        for name in [exe, library] {
            std::fs::write(root.join(name), b"fixture").unwrap();
            files.push(MuxFile {
                filename: name.into(),
                sha256: format!("{:x}", Sha256::digest(b"fixture")),
                bytes: 7,
            });
        }
        let components = MuxComponents {
            directory: root.clone(),
            ffmpeg: root.join(exe),
            ffprobe: None,
            files,
        };
        components.recheck().unwrap();
        std::fs::write(root.join("ffmpeg/bin/unsealed.dll"), b"unknown").unwrap();
        assert!(components.recheck().is_err());
        std::fs::remove_file(root.join("ffmpeg/bin/unsealed.dll")).unwrap();
        std::fs::write(root.join(library), b"mutated").unwrap();
        assert!(components.recheck().is_err());
        assert!(closed_path(&root, "../outside").is_err());
        assert!(closed_path(&root, "ffmpeg/../../outside").is_err());
        std::fs::remove_file(root.join(library)).unwrap();
        std::fs::remove_file(root.join(exe)).unwrap();
        std::fs::remove_dir(root.join("ffmpeg/bin")).unwrap();
        std::fs::remove_dir(root.join("ffmpeg/lib")).unwrap();
        std::fs::remove_dir(root.join("ffmpeg")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
