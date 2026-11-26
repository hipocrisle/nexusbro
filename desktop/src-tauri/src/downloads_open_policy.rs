//! Политика открытия скачанного файла. Чистая проверка без побочных эффектов:
//! путь приходит только из сохранённой записи загрузки, никогда от frontend.
//! Открываются безопасные документы/изображения/архивы по явному клику;
//! исполняемые и системные типы блокируются, обходы через trailing dot/space
//! и NTFS ADS закрыты, на Linux дополнительно смотрим execute-биты.

use std::path::{Path, PathBuf};

/// Расширения, которые нельзя открывать из браузера даже по явному клику.
const DANGEROUS: &[&str] = &[
    // Универсальные исполняемые
    "exe", "msi", "msix", "com", "scr", "pif", "gadget", "application", "msp", "mst",
    "dll", "sys", "cpl", "msc", "sct",
    // Скрипты и интерпретаторы
    "bat", "cmd", "ps1", "psm1", "psd1", "vbs", "vbe", "js", "jse", "wsf", "wsh",
    "jar", "sh", "py", "pl", "rb", "vb",
    // Системные ассоциации/ярлыки/пакеты
    "lnk", "url", "hta", "scf", "reg", "inf", "ins", "isp",
    "app", "apk", "desktop", "appimage", "run", "bin", "deb", "rpm",
];

fn blocked_extension(ext: &str) -> bool {
    let ext = ext.to_lowercase();
    // Windows трактует "name.exe." и "name.exe " как исполняемые: сравниваем
    // без завершающих точек/пробелов, чтобы guard нельзя было обойти.
    let ext = ext.trim_end_matches(['.', ' ']);
    DANGEROUS.contains(&ext)
}

/// Проверяет сохранённую запись и возвращает канонический путь, который можно
/// открыть. Ошибки нейтральные, без сырых путей.
pub fn check_open_target(status: &str, stored_path: &str) -> Result<PathBuf, String> {
    if status != "Готово" {
        return Err("Файл ещё не загружен полностью".into());
    }
    if stored_path.is_empty() {
        return Err("Путь к файлу неизвестен".into());
    }
    let path = Path::new(stored_path);
    if !path.is_absolute() {
        return Err("Путь к файлу недействителен".into());
    }
    // Trailing dot/space в имени файла — обход ассоциаций на Windows.
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.ends_with('.') || name.ends_with(' ') {
            return Err("Имя файла недопустимо для открытия".into());
        }
        // NTFS ADS: двоеточие в имени открывает альтернативный поток.
        if name.contains(':') {
            return Err("Имя файла недопустимо для открытия".into());
        }
    } else {
        return Err("Путь к файлу недействителен".into());
    }
    // Проверяем ИСХОДНЫЙ путь до canonicalize: canonicalize сам разрешает
    // symlink, поэтому проверка после него была бы недостижимой.
    let original_meta = std::fs::symlink_metadata(path)
        .map_err(|_| "Файл не найден: возможно, он был удалён или перемещён".to_string())?;
    if original_meta.file_type().is_symlink() {
        return Err("Открыть можно только обычный файл".into());
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|_| "Файл не найден: возможно, он был удалён или перемещён".to_string())?;
    let meta = std::fs::metadata(&canonical)
        .map_err(|_| "Файл не найден: возможно, он был удалён или перемещён".to_string())?;
    if !meta.is_file() {
        return Err("Открыть можно только обычный файл".into());
    }
    let ext = canonical
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if blocked_extension(ext) {
        return Err(format!(
            "Открытие файлов этого типа заблокировано как потенциально опасное. \
             Если доверяете источнику, откройте файл вручную из папки."
        ));
    }
    // Linux: исполняемый бит на скачанном файле — не открываем из браузера.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Err("Файл помечен как исполняемый и не будет открыт из браузера. \
                        Откройте его вручную из папки, если доверяете источнику.".into());
        }
    }
    Ok(canonical)
}

/// Проверка пути папки для «Показать в папке»: только существующая абсолютная
/// папка; имя не должно выглядеть как опция командной строки.
pub fn check_folder_target(stored_path: &str) -> Result<PathBuf, String> {
    if stored_path.is_empty() {
        return Err("Путь к файлу неизвестен".into());
    }
    let path = Path::new(stored_path);
    if !path.is_absolute() {
        return Err("Путь к файлу недействителен".into());
    }
    let parent = path
        .parent()
        .filter(|p| p.is_dir())
        .ok_or("Папка загрузки не найдена: файла могло уже не быть на диске")?;
    if parent.to_string_lossy().starts_with('-') {
        return Err("Путь к папке недопустим".into());
    }
    Ok(parent.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn temp_file(name: &str) -> (tempdir::TempDir, PathBuf) {
        let dir = tempdir::TempDir::new().unwrap();
        let path = dir.path().join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"data").unwrap();
        (dir, path)
    }

    mod tempdir {
        use std::path::{Path, PathBuf};
        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new() -> std::io::Result<Self> {
                let mut p = std::env::temp_dir();
                p.push(format!("nb-open-policy-{}-{}", std::process::id(),
                               std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
                std::fs::create_dir_all(&p)?;
                Ok(Self(p))
            }
            pub fn path(&self) -> &Path { &self.0 }
        }
        impl Drop for TempDir {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
    }

    #[test]
    fn safe_document_opens() {
        let (_d, p) = temp_file("report.pdf");
        assert!(check_open_target("Готово", p.to_str().unwrap()).is_ok());
    }

    #[test]
    fn requires_done_status() {
        let (_d, p) = temp_file("a.txt");
        assert!(check_open_target("Загружается", p.to_str().unwrap()).is_err());
    }

    #[test]
    fn relative_rejected() {
        assert!(check_open_target("Готово", "relative/file.txt").is_err());
    }

    #[test]
    fn missing_file_rejected() {
        assert!(check_open_target("Готово", "/nonexistent/nb/none.txt").is_err());
    }

    #[test]
    fn executable_extension_blocked_even_with_trailing_dot() {
        let (_d, p) = temp_file("run.EXE.");
        assert!(check_open_target("Готово", p.to_str().unwrap()).is_err());
    }

    #[test]
    fn windows_dangerous_types_blocked() {
        for ext in ["url", "hta", "scf", "msc", "cpl", "reg", "pif", "psm1", "psd1", "py", "pl", "rb", "vb", "sct", "inf", "ins", "isp", "application", "msp", "mst"] {
            let (_d, p) = temp_file(&format!("x.{ext}"));
            assert!(check_open_target("Готово", p.to_str().unwrap()).is_err(), "ext {ext}");
        }
    }

    #[test]
    fn ads_colon_rejected() {
        let (_d, p) = temp_file("doc.txt:stream");
        assert!(check_open_target("Готово", p.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn executable_bit_blocked_on_linux() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, p) = temp_file("script.txt");
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms).unwrap();
        assert!(check_open_target("Готово", p.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_rejected_before_canonicalize() {
        let (d, p) = temp_file("real.txt");
        let link = d.path().join("alias.txt");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(check_open_target("Готово", link.to_str().unwrap()).is_err());
    }

    #[test]
    fn folder_parent_ok_and_option_like_rejected() {
        let (d, p) = temp_file("a.txt");
        assert_eq!(check_folder_target(p.to_str().unwrap()).unwrap(), d.path());
        assert!(check_folder_target("-rf/file.txt").is_err());
    }
}
