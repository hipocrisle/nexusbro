//! Route-bound local WebM download. Call only on an owned CEF background job.
//! URLs/headers never become filenames, log strings, helper argv or disk JSON.
use crate::{
    tab_routes::NativeRoute,
    youtube_light_components::MuxComponents,
    youtube_light_extract::{MediaFormat, MediaKind},
    youtube_light_mux,
};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
const GIB: u64 = 1024 * 1024 * 1024;
const RESERVE: u64 = 512 * 1024 * 1024;
const RANGE: u64 = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadError {
    InvalidInput,
    UnsupportedFormat,
    RouteUnavailable,
    Network,
    InvalidRange,
    Oversized,
    InsufficientSpace,
    Storage,
    Cancelled,
    TimedOut,
    MuxUnavailable,
    MuxFailed,
    DestinationExists,
    PublishUnsupported,
    CleanupUnknown,
}
impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CleanupUnknown => "Завершение загрузки или очистки не подтверждено",
            Self::DestinationExists => "Файл назначения уже существует",
            Self::InsufficientSpace => "Недостаточно свободного места для загрузки",
            Self::UnsupportedFormat => {
                "Этот формат не поддерживается для загрузки без преобразования"
            }
            _ => "Не удалось безопасно завершить загрузку видео",
        })
    }
}
impl std::error::Error for DownloadError {}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_total_bytes: u64,
    pub min_free_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_total_bytes: 8 * GIB,
            min_free_bytes: RESERVE,
        }
    }
}
impl Limits {
    fn valid(self) -> bool {
        self.max_total_bytes > 0
            && self.max_total_bytes <= 32 * GIB
            && self.min_free_bytes >= RESERVE
            && self.min_free_bytes <= 32 * GIB
    }
}
pub enum Selection<'a> {
    Audio(&'a MediaFormat),
    Video {
        video: &'a MediaFormat,
        audio: &'a MediaFormat,
    },
    Muxed(&'a MediaFormat),
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Downloading,
    Muxing,
    Verifying,
    Publishing,
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub bytes: u64,
    pub total_bytes: Option<u64>,
    pub mux_bytes: u64,
}
#[derive(Debug, serde::Serialize)]
pub struct Completed {
    pub bytes: u64,
    pub sha256: String,
    pub container: &'static str,
}
pub struct Target {
    directory: PathBuf,
    filename: String,
}
fn directory_safe(directory: &Path) -> bool {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        match directory.components().next() {
            Some(Component::Prefix(p))
                if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
            _ => return false,
        }
    }
    directory.ancestors().all(|p| {
        std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    })
}
fn filename_safe(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 200
        || name.starts_with('.')
        || name.starts_with('-')
        || name.ends_with(' ')
        || !name.ends_with(".webm")
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '"' | '<' | '>' | '|' | '?' | '*')
        })
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
impl Target {
    pub fn new(directory: PathBuf, filename: String) -> Result<Self, DownloadError> {
        if !directory_safe(&directory) || !filename_safe(&filename) {
            return Err(DownloadError::InvalidInput);
        }
        Ok(Self {
            directory,
            filename,
        })
    }
}
fn free_bytes(directory: &Path) -> Result<u64, DownloadError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(directory.as_os_str().as_bytes())
            .map_err(|_| DownloadError::Storage)?;
        let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
            return Err(DownloadError::Storage);
        }
        let info = unsafe { info.assume_init() };
        return (info.f_bavail as u64)
            .checked_mul(info.f_frsize as u64)
            .ok_or(DownloadError::Storage);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetDiskFreeSpaceExW(
                path: *const u16,
                available: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
        }
        let path = directory
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut available = 0;
        if unsafe {
            GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(DownloadError::Storage);
        }
        return Ok(available);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Err(DownloadError::Storage)
    }
}
fn space(directory: &Path, limits: Limits, remaining: u64) -> Result<(), DownloadError> {
    let needed = limits
        .min_free_bytes
        .checked_add(remaining)
        .ok_or(DownloadError::Oversized)?;
    if free_bytes(directory)? < needed {
        return Err(DownloadError::InsufficientSpace);
    }
    Ok(())
}
struct Stage {
    root: PathBuf,
    retain: bool,
}
impl Stage {
    fn new(directory: &Path) -> Result<Self, DownloadError> {
        let mut random = [0u8; 16];
        OsRng
            .try_fill_bytes(&mut random)
            .map_err(|_| DownloadError::Storage)?;
        let suffix = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let root = directory.join(format!(".nexusbro-download-{suffix}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&root).map_err(|_| DownloadError::Storage)?;
        Ok(Self {
            root,
            retain: false,
        })
    }
    fn cleanup(&mut self) -> Result<(), DownloadError> {
        for name in ["video.webm", "audio.webm", "output.webm"] {
            match std::fs::remove_file(self.root.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    self.retain = true;
                    return Err(DownloadError::CleanupUnknown);
                }
            }
        }
        match std::fs::remove_dir(&self.root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                self.retain = true;
                return Err(DownloadError::CleanupUnknown);
            }
        }
        self.retain = true;
        Ok(())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if !self.retain {
            let _ = self.cleanup();
        }
    }
}
fn new_file(path: &Path) -> Result<File, DownloadError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|_| DownloadError::Storage)
}
fn vp9(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|v| {
        v == "vp9"
            || v.strip_prefix("vp09.").is_some_and(|s| {
                !s.is_empty() && s.len() <= 48 && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            })
    })
}
fn opus(format: &MediaFormat) -> bool {
    format.ext == "webm" && format.acodec.as_deref() == Some("opus")
}
pub(crate) fn supports_audio(a: &MediaFormat) -> bool {
    a.kind == MediaKind::AudioOnly && opus(a) && a.vcodec.is_none()
}
pub(crate) fn supports_video(v: &MediaFormat) -> bool {
    v.ext == "webm" && v.kind == MediaKind::VideoOnly && vp9(&v.vcodec) && v.acodec.is_none()
}
fn selection_valid(selection: &Selection<'_>) -> bool {
    match selection {
        Selection::Audio(a) => supports_audio(a),
        Selection::Video { video, audio } => supports_video(video) && supports_audio(audio),
        Selection::Muxed(v) => v.kind == MediaKind::Muxed && opus(v) && vp9(&v.vcodec),
    }
}
#[derive(Default)]
struct Budget {
    needs_mux: bool,
    announced: u64,
    streams: usize,
    received: u64,
    mux_bytes: u64,
}
fn parse_range(text: &str) -> Result<(u64, u64, u64), DownloadError> {
    let raw = text
        .strip_prefix("bytes ")
        .ok_or(DownloadError::InvalidRange)?;
    let (interval, total) = raw.split_once('/').ok_or(DownloadError::InvalidRange)?;
    let (start, end) = interval
        .split_once('-')
        .ok_or(DownloadError::InvalidRange)?;
    let parse = |s: &str| {
        if !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse::<u64>().map_err(|_| DownloadError::InvalidRange)
        } else {
            Err(DownloadError::InvalidRange)
        }
    };
    let (start, end, total) = (parse(start)?, parse(end)?, parse(total)?);
    if total == 0 || start > end || end >= total {
        return Err(DownloadError::InvalidRange);
    }
    Ok((start, end, total))
}
// Strict additional test CA for the fixed disposable fixture only. Normal
// certificate/hostname validation stays enabled; no production host is affected.
fn fixture_trust(builder:reqwest::ClientBuilder,format:&MediaFormat)->Result<reqwest::ClientBuilder,DownloadError>{
    if std::env::var("GITHUB_ACTIONS").as_deref()!=Ok("true")||std::env::var_os("NEXUSBRO_TEST_DATA").is_none(){return Ok(builder);}
    let url=reqwest::Url::parse(format.url.as_str()).map_err(|_|DownloadError::InvalidRange)?;
    if url.host_str()!=Some("fixture.googlevideo.com"){return Ok(builder);}
    let Some(path)=std::env::var_os("NEXUSBRO_TEST_YOUTUBE_DOWNLOAD_CA") else{return Ok(builder);};
    let path=PathBuf::from(path);
    let metadata=std::fs::symlink_metadata(&path).map_err(|_|DownloadError::Storage)?;
    if !path.is_absolute()||!metadata.is_file()||metadata.file_type().is_symlink()||metadata.len()>16384{return Err(DownloadError::Storage);}
    let pem=std::fs::read(path).map_err(|_|DownloadError::Storage)?;
    let expected=std::env::var("NEXUSBRO_TEST_YOUTUBE_DOWNLOAD_CA_SHA256").map_err(|_|DownloadError::Storage)?;
    use sha2::Digest;
    let actual=format!("{:x}",sha2::Sha256::digest(&pem));
    if expected.len()!=64||expected!=actual{return Err(DownloadError::Storage);}
    let cert=reqwest::Certificate::from_pem(&pem).map_err(|_|DownloadError::Storage)?;
    Ok(builder.add_root_certificate(cert))
}
async fn stream(
    format: &MediaFormat,
    route: &NativeRoute,
    path: &Path,
    directory: &Path,
    limits: Limits,
    budget: &Mutex<Budget>,
) -> Result<(), DownloadError> {
    let proxy = reqwest::Proxy::all(&route.proxy).map_err(|_| DownloadError::RouteUnavailable)?;
    let builder = reqwest::Client::builder()
        .no_proxy()
        .proxy(proxy)
        .no_gzip()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45));
    let client = fixture_trust(builder,format)?
        .build()
        .map_err(|_| DownloadError::RouteUnavailable)?;
    let mut file = new_file(path)?;
    let mut offset = 0u64;
    let mut expected = None;
    let mut tag = None;
    loop {
        let end = offset
            .checked_add(RANGE - 1)
            .ok_or(DownloadError::Oversized)?;
        let mut request = client.get(format.url.as_str());
        for (name, value) in format.headers.iter() {
            request = request.header(name, value);
        }
        request = request
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"));
        let mut response = request.send().await.map_err(|_| DownloadError::Network)?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
            || response
                .headers()
                .contains_key(reqwest::header::CONTENT_ENCODING)
        {
            return Err(DownloadError::InvalidRange);
        }
        let (start, actual_end, total) = parse_range(
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .ok_or(DownloadError::InvalidRange)?,
        )?;
        if start != offset
            || actual_end != end.min(total - 1)
            || response.content_length() != Some(actual_end - start + 1)
            || expected.is_some_and(|n| n != total)
        {
            return Err(DownloadError::InvalidRange);
        }
        let current_tag = response
            .headers()
            .get(reqwest::header::ETAG)
            .map(|v| v.as_bytes().to_vec());
        if offset == 0 {
            tag = current_tag;
            expected = Some(total);
            let mut b = budget.lock().map_err(|_| DownloadError::Storage)?;
            b.announced = b
                .announced
                .checked_add(total)
                .filter(|n| *n <= limits.max_total_bytes)
                .ok_or(DownloadError::Oversized)?;
            b.streams += 1;
        } else if tag != current_tag {
            return Err(DownloadError::InvalidRange);
        }
        budget_space(directory, limits, budget)?;
        let mut count = 0u64;
        while let Some(chunk) = response.chunk().await.map_err(|_| DownloadError::Network)? {
            count = count
                .checked_add(chunk.len() as u64)
                .ok_or(DownloadError::Oversized)?;
            if count > actual_end - start + 1 || chunk.len() > 1024 * 1024 {
                return Err(DownloadError::InvalidRange);
            }
            budget_space(directory, limits, budget)?;
            file.write_all(&chunk).map_err(|_| DownloadError::Storage)?;
            let mut b = budget.lock().map_err(|_| DownloadError::Storage)?;
            b.received = b
                .received
                .checked_add(chunk.len() as u64)
                .filter(|n| *n <= limits.max_total_bytes)
                .ok_or(DownloadError::Oversized)?;
        }
        if count != actual_end - start + 1 {
            return Err(DownloadError::InvalidRange);
        }
        offset = actual_end + 1;
        if offset == total {
            break;
        }
    }
    file.sync_all().map_err(|_| DownloadError::Storage)?;
    Ok(())
}
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}
fn budget_space(
    directory: &Path,
    limits: Limits,
    budget: &Mutex<Budget>,
) -> Result<(), DownloadError> {
    let remaining = {
        let b = budget.lock().map_err(|_| DownloadError::Storage)?;
        b.announced
            .checked_mul(if b.needs_mux { 2 } else { 1 })
            .ok_or(DownloadError::Oversized)?
            .saturating_sub(b.received)
    };
    space(directory, limits, remaining)
}
fn is_cancelled(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow() || cancel.has_changed().is_err()
}
fn hash_webm(
    path: &Path,
    limit: u64,
    cancel: &watch::Receiver<bool>,
) -> Result<(u64, String), DownloadError> {
    if limit < 4 {
        return Err(DownloadError::Oversized);
    }
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
        return Err(DownloadError::Storage);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| DownloadError::Storage)?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)
        .map_err(|_| DownloadError::UnsupportedFormat)?;
    if header != [0x1a, 0x45, 0xdf, 0xa3] {
        return Err(DownloadError::UnsupportedFormat);
    }
    let mut hash = Sha256::new();
    hash.update(header);
    let mut total = 4u64;
    let mut buffer = [0u8; 65536];
    let deadline = std::time::Instant::now() + Duration::from_secs(900);
    loop {
        if is_cancelled(cancel) {
            return Err(DownloadError::Cancelled);
        }
        if std::time::Instant::now() >= deadline {
            return Err(DownloadError::TimedOut);
        }
        let n = file.read(&mut buffer).map_err(|_| DownloadError::Storage)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .filter(|n| *n <= limit)
            .ok_or(DownloadError::Oversized)?;
        hash.update(&buffer[..n]);
    }
    file.sync_all().map_err(|_| DownloadError::Storage)?;
    Ok((total, format!("{:x}", hash.finalize())))
}
fn publish(source: &Path, target: &Target) -> Result<(), DownloadError> {
    if !directory_safe(&target.directory) {
        return Err(DownloadError::InvalidInput);
    }
    let destination = target.directory.join(&target.filename);
    std::fs::hard_link(source, &destination).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            DownloadError::DestinationExists
        } else {
            DownloadError::PublishUnsupported
        }
    })?;
    #[cfg(unix)]
    {
        File::open(&target.directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| DownloadError::CleanupUnknown)?;
    }
    Ok(())
}
/// Parent owns job/thread/route lease. Keep this future until it finishes; a
/// dropped/panicked job must be latched as cleanup-unknown by its supervisor.
pub async fn run(
    selection: Selection<'_>,
    route: &NativeRoute,
    target: &Target,
    mux: Option<&MuxComponents>,
    limits: Limits,
    mut cancel: watch::Receiver<bool>,
    mut progress: impl FnMut(Progress) -> bool + Send,
) -> Result<Completed, DownloadError> {
    if !limits.valid() || !directory_safe(&target.directory) {
        return Err(DownloadError::InvalidInput);
    }
    match std::fs::symlink_metadata(target.directory.join(&target.filename)) {
        Ok(_) => return Err(DownloadError::DestinationExists),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(DownloadError::Storage),
    }
    if !route.validate() {
        return Err(DownloadError::RouteUnavailable);
    }
    if !selection_valid(&selection) {
        return Err(DownloadError::UnsupportedFormat);
    }
    if matches!(&selection, Selection::Video { .. }) && mux.is_none() {
        return Err(DownloadError::MuxUnavailable);
    }
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(DownloadError::Cancelled);
    }
    space(&target.directory, limits, 0)?;
    let mut stage = Stage::new(&target.directory)?;
    let video_path = stage.root.join("video.webm");
    let audio_path = stage.root.join("audio.webm");
    let output_path = stage.root.join("output.webm");
    let budget = Arc::new(Mutex::new(Budget {
        needs_mux: matches!(&selection, Selection::Video { .. }),
        ..Budget::default()
    }));
    let downloading = {
        let transfer = async {
            match &selection {
                Selection::Audio(a) => {
                    stream(a, route, &audio_path, &target.directory, limits, &budget).await
                }
                Selection::Muxed(v) => {
                    stream(v, route, &video_path, &target.directory, limits, &budget).await
                }
                Selection::Video { video, audio } => {
                    tokio::try_join!(
                        stream(
                            video,
                            route,
                            &video_path,
                            &target.directory,
                            limits,
                            &budget
                        ),
                        stream(
                            audio,
                            route,
                            &audio_path,
                            &target.directory,
                            limits,
                            &budget
                        )
                    )?;
                    Ok(())
                }
            }
        };
        tokio::pin!(transfer);
        let deadline = tokio::time::sleep(TIMEOUT);
        tokio::pin!(deadline);
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {biased;_=cancelled(&mut cancel)=>break Err(DownloadError::Cancelled),_=&mut deadline=>break Err(DownloadError::TimedOut),result=&mut transfer=>break result,_=tick.tick()=>{let p=match budget.lock(){Ok(b)=>Progress{phase:Phase::Downloading,bytes:b.received,total_bytes:(b.streams==if matches!(&selection,Selection::Video{..}){2}else{1}).then_some(b.announced),mux_bytes:0},Err(_)=>break Err(DownloadError::Storage)};if !progress(p){break Err(DownloadError::Cancelled);}}}
        }
    };
    let result = (async {
        downloading?;
        let selected = match &selection {
            Selection::Audio(_) => &audio_path,
            Selection::Muxed(_) => &video_path,
            Selection::Video { .. } => {
                hash_webm(&video_path, limits.max_total_bytes, &cancel)?;
                hash_webm(&audio_path, limits.max_total_bytes, &cancel)?;
                let mut stop_reason = None;
                let merged = youtube_light_mux::mux(
                    mux.ok_or(DownloadError::MuxUnavailable)?,
                    &video_path,
                    Some(&audio_path),
                    &output_path,
                    cancel.clone(),
                    |p| {
                        let size = std::fs::metadata(&output_path)
                            .map(|m| m.len())
                            .unwrap_or(0);
                        if size > limits.max_total_bytes {
                            stop_reason = Some(DownloadError::Oversized);
                            return false;
                        }
                        if let Err(error) = space(&target.directory, limits, 0) {
                            stop_reason = Some(error);
                            return false;
                        }
                        let bytes = budget.lock().map(|b| b.received).unwrap_or(0);
                        progress(Progress {
                            phase: Phase::Muxing,
                            bytes,
                            total_bytes: Some(bytes),
                            mux_bytes: p.bytes,
                        })
                    },
                )
                .await;
                if let Err(error) = merged {
                    if error == youtube_light_mux::MuxError::CleanupUnknown {
                        return Err(DownloadError::CleanupUnknown);
                    }
                    return Err(stop_reason.unwrap_or(
                        if error == youtube_light_mux::MuxError::Cancelled {
                            DownloadError::Cancelled
                        } else {
                            DownloadError::MuxFailed
                        },
                    ));
                }
                &output_path
            }
        };
        if is_cancelled(&cancel) {
            return Err(DownloadError::Cancelled);
        }
        let bytes = budget.lock().map_err(|_| DownloadError::Storage)?.received;
        if !progress(Progress {
            phase: Phase::Verifying,
            bytes,
            total_bytes: Some(bytes),
            mux_bytes: 0,
        }) {
            return Err(DownloadError::Cancelled);
        }
        let (size, sha256) = hash_webm(selected, limits.max_total_bytes, &cancel)?;
        if is_cancelled(&cancel)
            || !progress(Progress {
                phase: Phase::Publishing,
                bytes,
                total_bytes: Some(bytes),
                mux_bytes: size,
            })
        {
            return Err(DownloadError::Cancelled);
        }
        publish(selected, target)?;
        Ok(Completed {
            bytes: size,
            sha256,
            container: "webm",
        })
    })
    .await;
    if matches!(result, Err(DownloadError::CleanupUnknown)) {
        stage.retain = true;
        return result;
    }
    stage.cleanup()?;
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_parser_is_exact_and_never_repairs_unknown_sizes() {
        assert_eq!(parse_range("bytes 0-7/20").unwrap(), (0, 7, 20));
        for bad in [
            "bytes 0-7/*",
            "bytes 0-20/20",
            "bytes 8-7/20",
            "bytes +0-7/20",
            "bytes 0-7/0",
            "bytes 0-7/20 junk",
        ] {
            assert!(parse_range(bad).is_err());
        }
    }
    #[test]
    fn filename_and_budget_guards_refuse_traversal_or_silent_container_conversion() {
        for bad in [
            "../x.webm",
            "x.opus",
            "CON.webm",
            "LPT1.webm",
            ".hidden.webm",
            "x/y.webm",
            "x\\y.webm",
            "x\n.webm",
        ] {
            assert!(!filename_safe(bad));
        }
        assert!(filename_safe("Видео 1080p.webm"));
        assert!(Limits::default().valid());
        assert!(!Limits {
            max_total_bytes: 33 * GIB,
            min_free_bytes: RESERVE
        }
        .valid());
        assert!(!Limits {
            max_total_bytes: GIB,
            min_free_bytes: 0
        }
        .valid());
    }
    #[test]
    fn atomic_publication_never_overwrites_and_stage_cleanup_is_bounded() {
        let root = std::env::temp_dir().join(format!(
            "nb-download-test-{}-{}",
            std::process::id(),
            rand_core::OsRng.next_u64()
        ));
        std::fs::create_dir(&root).unwrap();
        let target = Target::new(root.clone(), "video.webm".into()).unwrap();
        let mut stage = Stage::new(&root).unwrap();
        let source = stage.root.join("video.webm");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(root.join("video.webm"), b"existing").unwrap();
        assert_eq!(
            publish(&source, &target).unwrap_err(),
            DownloadError::DestinationExists
        );
        assert_eq!(std::fs::read(root.join("video.webm")).unwrap(), b"existing");
        stage.cleanup().unwrap();
        std::fs::remove_file(root.join("video.webm")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn output_directory_symlink_is_not_a_trusted_target() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "nb-download-link-{}-{}",
            std::process::id(),
            rand_core::OsRng.next_u64()
        ));
        std::fs::create_dir(&root).unwrap();
        symlink(&root, root.join("alias")).unwrap();
        assert!(Target::new(root.join("alias"), "video.webm".into()).is_err());
        std::fs::remove_file(root.join("alias")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
