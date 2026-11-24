//! Controlled anonymous single-video extraction. Parent owns component digest
//! verification, route lifetime and whole process-tree/job ownership. This
//! module reaps the direct child; it does not claim descendant containment.
use crate::{tab_routes::NativeRoute, youtube_light::VideoId};
use std::{
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::watch,
};
use zeroize::Zeroizing;

pub const STDOUT_LIMIT: usize = 4 * 1024 * 1024;
pub const STDERR_LIMIT: usize = 64 * 1024;
pub const EXECUTION_TIMEOUT: Duration = Duration::from_secs(45);
pub const REAP_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractError {
    InvalidComponents,
    InvalidRoute,
    Cancelled,
    SpawnFailed,
    OutputLimit,
    ReadFailed,
    WaitFailed,
    ProcessFailed,
    UnsupportedInvocation,
    HttpForbidden,
    RateLimited,
    TlsFailure,
    ChallengeRequired,
    VideoUnavailable,
    HttpBadRequest,
    HttpNotFound,
    HttpServerError,
    NetworkFailure,
    RuntimeFailure,
    ChallengeSolverUnavailable,
    FormatUnavailable,
    FfmpegUnavailable,
    ExtractionFailure,
    TimedOut,
    ReapFailed,
    EmptyOutput,
}
impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidComponents => "Компоненты извлечения недоступны",
            Self::InvalidRoute => "Путь видео недоступен",
            Self::Cancelled => "Извлечение отменено",
            Self::SpawnFailed => "Не удалось запустить извлечение",
            Self::OutputLimit => "Ответ извлечения превышает лимит",
            Self::ReadFailed => "Не удалось получить ответ извлечения",
            Self::WaitFailed => "Не удалось дождаться извлечения",
            Self::ProcessFailed => "Видео недоступно для анонимного извлечения",
            Self::UnsupportedInvocation => {
                "Компонент извлечения не поддерживает необходимые параметры"
            }
            Self::HttpForbidden => "Источник отклонил запрос видео",
            Self::RateLimited => "Источник временно ограничил частоту запросов",
            Self::TlsFailure => "Не удалось проверить защищённое соединение с источником",
            Self::ChallengeRequired => {
                "Для этого видео требуется дополнительная проверка источника"
            }
            Self::VideoUnavailable => "Это видео недоступно для анонимного просмотра",
            Self::HttpBadRequest => "Источник отклонил параметры запроса",
            Self::HttpNotFound => "Источник видео не найден",
            Self::HttpServerError => "Ошибка сервера источника видео",
            Self::NetworkFailure => "Сетевое соединение извлечения прервано",
            Self::RuntimeFailure => "Не удалось выполнить JavaScript проверки видео",
            Self::ChallengeSolverUnavailable => "Компонент проверки JavaScript недоступен",
            Self::FormatUnavailable => "Запрошенный формат видео недоступен",
            Self::FfmpegUnavailable => "Для запрошенной операции нужен компонент обработки медиа",
            Self::ExtractionFailure => "Не удалось разобрать ответ источника видео",
            Self::TimedOut => "Истекло время извлечения",
            Self::ReapFailed => "Завершение процесса извлечения не подтверждено",
            Self::EmptyOutput => "Извлечение не вернуло данные",
        })
    }
}
impl std::error::Error for ExtractError {}
/// Only trusted parent code constructs this after verifying pinned component
/// digests and rejecting writable/untrusted replacements. No web deserializer.
pub struct Components {
    extractor: PathBuf,
    js_runtime: PathBuf,
}
impl fmt::Debug for Components {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Components([parent supplied])")
    }
}
fn executable_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some()
        && std::fs::symlink_metadata(path)
            .is_ok_and(|m| m.file_type().is_file() && !m.file_type().is_symlink())
}
impl Components {
    pub fn new(extractor: PathBuf, js_runtime: PathBuf) -> Result<Self, ExtractError> {
        if !executable_path(&extractor) || !executable_path(&js_runtime) {
            return Err(ExtractError::InvalidComponents);
        }
        Ok(Self {
            extractor,
            js_runtime,
        })
    }
}
fn command(
    components: &Components,
    video: &VideoId,
    route: &NativeRoute,
) -> Result<Command, ExtractError> {
    if !route.validate() {
        return Err(ExtractError::InvalidRoute);
    }
    if !executable_path(&components.extractor) || !executable_path(&components.js_runtime) {
        return Err(ExtractError::InvalidComponents);
    }
    let mut c = Command::new(&components.extractor);
    c.args([
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-remote-components",
        "--no-cache-dir",
        "--no-playlist",
        "--skip-download",
        "--dump-single-json",
        "--no-progress",
        "--no-cookies",
        "--no-cookies-from-browser",
        "--no-js-runtimes",
    ])
    .arg("--js-runtimes")
    .arg(format!(
        "deno:{}",
        components
            .js_runtime
            .to_str()
            .ok_or(ExtractError::InvalidComponents)?
    ))
    .arg("--proxy")
    .arg(&route.proxy)
    .args([
        "--socket-timeout",
        "5",
        "--retries",
        "1",
        "--fragment-retries",
        "0",
    ])
    .arg("--")
    .arg(video.watch_url());
    // No HTTP_PROXY/HTTPS_PROXY/ALL_PROXY/NO_PROXY, custom certificates, token,
    // cookie, Python/plugin/Deno configuration or inherited secret variables.
    c.env_clear();
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            c.env(key, value);
        }
    }
    // An explicit runtime path is mandatory; PATH is not inherited or searched.
    c.env("DENO_NO_UPDATE_CHECK", "1")
        .env("DENO_NO_PROMPT", "1");
    c.current_dir(
        components
            .extractor
            .parent()
            .ok_or(ExtractError::InvalidComponents)?,
    );
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        c.creation_flags(0x08000000);
    }
    Ok(c)
}
async fn bounded<R: AsyncRead + Unpin>(
    mut source: R,
    limit: usize,
    retain: bool,
) -> Result<Zeroizing<Vec<u8>>, ExtractError> {
    let mut kept = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 8192]);
    let mut total = 0usize;
    loop {
        let read = source
            .read(&mut *chunk)
            .await
            .map_err(|_| ExtractError::ReadFailed)?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read).ok_or(ExtractError::OutputLimit)?;
        if total > limit {
            return Err(ExtractError::OutputLimit);
        }
        if retain {
            kept.extend_from_slice(&chunk[..read]);
        }
    }
    Ok(kept)
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
/// Fixed diagnostic vocabulary only. Flags are hints, never route authority.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct FailureFingerprint {
    pub exit_code: Option<i32>,
    pub stderr_bytes: usize,
    pub usage: bool,
    pub http_400: bool,
    pub http_403: bool,
    pub http_404: bool,
    pub http_429: bool,
    pub http_5xx: bool,
    pub tls: bool,
    pub network: bool,
    pub runtime: bool,
    pub solver: bool,
    pub format_unavailable: bool,
    pub ffmpeg_missing: bool,
    pub extractor: bool,
    pub error_prefix: bool,
    pub warning_prefix: bool,
}
#[derive(Debug)]
pub struct ExtractFailure {
    pub error: ExtractError,
    pub diagnostic: FailureFingerprint,
}
impl fmt::Display for ExtractFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}
impl std::error::Error for ExtractFailure {}
fn fingerprint(stderr: &[u8], exit_code: Option<i32>) -> FailureFingerprint {
    let has = |p: &[u8]| stderr.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p));
    let any = |patterns: &[&[u8]]| patterns.iter().any(|p| has(p));
    FailureFingerprint {
        exit_code,
        stderr_bytes: stderr.len(),
        usage: any(&[
            b"usage:",
            b"no such option:",
            b"unrecognized arguments:",
            b"invalid js runtime",
        ]),
        http_400: has(b"http error 400"),
        http_403: has(b"http error 403"),
        http_404: has(b"http error 404"),
        http_429: has(b"http error 429"),
        http_5xx: has(b"http error 5"),
        tls: any(&[
            b"certificate",
            b"sslerror",
            b"ssl:",
            b"tls handshake",
            b"ssl: unexpected_eof",
            b"sslcertverificationerror",
        ]),
        network: any(&[
            b"connection refused",
            b"connection reset",
            b"connection aborted",
            b"connection closed",
            b"remote end closed connection",
            b"unexpected eof",
            b"timed out",
            b"timeout",
            b"proxyerror",
            b"proxy error",
            b"tunnel connection failed",
            b"name resolution",
            b"getaddrinfo failed",
            b"nodename nor servname",
            b"network is unreachable",
            b"incompleteread",
        ]),
        runtime: any(&[
            b"error running deno process",
            b"no supported javascript runtime",
            b"javascript runtime could not",
            b"javascript runtime was not found",
            b"failed to get cache directory",
            b"failed to determine cache directory",
        ]),
        solver: any(&[
            b"ejs script",
            b"yt-dlp-ejs",
            b"challenge solver script",
            b"challenge provider",
        ]),
        format_unavailable: any(&[
            b"requested format is not available",
            b"no video formats found",
            b"no downloadable video formats",
        ]),
        ffmpeg_missing: any(&[
            b"ffmpeg is not installed",
            b"ffmpeg not found",
            b"ffmpeg is not available",
        ]),
        extractor: any(&[
            b"unable to extract",
            b"unable to download webpage",
            b"unable to download api",
            b"failed to extract",
        ]),
        error_prefix: has(b"error:"),
        warning_prefix: has(b"warning:"),
    }
}
/// Diagnostic hint only: remote text is not trusted authority, never returned.
/// Whole bounded stderr is available so a pipe-read boundary cannot hide a
/// phrase. No UTF-8 requirement and no additional lowercase/URL/string copies.
fn classify_failure(stderr: &[u8]) -> ExtractError {
    let contains = |pattern: &[u8]| {
        stderr
            .windows(pattern.len())
            .any(|part| part.eq_ignore_ascii_case(pattern))
    };
    if [
        b"no such option:".as_slice(),
        b"unrecognized arguments:",
        b"unrecognized option",
        b"invalid js runtime format",
        b"invalid proxy url",
    ]
    .iter()
    .any(|p| contains(p))
    {
        return ExtractError::UnsupportedInvocation;
    }
    if [
        b"certificate_verify_failed".as_slice(),
        b"certificate verify failed",
        b"ssl: certificate",
        b"tls handshake",
        b"unable to get local issuer certificate",
        b"certificate has expired",
    ]
    .iter()
    .any(|p| contains(p))
    {
        return ExtractError::TlsFailure;
    }
    if [
        b"http error 429".as_slice(),
        b"429: too many requests",
        b"rate limit exceeded",
        b"request rate limit",
    ]
    .iter()
    .any(|p| contains(p))
    {
        return ExtractError::RateLimited;
    }
    if (contains(b"sign in to confirm you") && contains(b"not a bot"))
        || [
            b"sign in to confirm you're not a bot".as_slice(),
            b"js challenge",
            b"javascript challenge",
            b"challenge solving failed",
            b"po token is required",
            b"requires a po token",
        ]
        .iter()
        .any(|p| contains(p))
    {
        return ExtractError::ChallengeRequired;
    }
    if [b"http error 403".as_slice(), b"403: forbidden"]
        .iter()
        .any(|p| contains(p))
    {
        return ExtractError::HttpForbidden;
    }
    if [
        b"video unavailable".as_slice(),
        b"private video",
        b"this video has been removed",
        b"this video is not available",
        b"sign in to confirm your age",
        b"members-only content",
        b"this video is only available",
    ]
    .iter()
    .any(|p| contains(p))
    {
        return ExtractError::VideoUnavailable;
    }
    let flags = fingerprint(stderr, None);
    if flags.usage {
        return ExtractError::UnsupportedInvocation;
    }
    if flags.http_400 {
        return ExtractError::HttpBadRequest;
    }
    if flags.http_404 {
        return ExtractError::HttpNotFound;
    }
    if flags.http_5xx {
        return ExtractError::HttpServerError;
    }
    if flags.runtime {
        return ExtractError::RuntimeFailure;
    }
    if flags.solver {
        return ExtractError::ChallengeSolverUnavailable;
    }
    if flags.tls {
        return ExtractError::TlsFailure;
    }
    if flags.network {
        return ExtractError::NetworkFailure;
    }
    if flags.format_unavailable {
        return ExtractError::FormatUnavailable;
    }
    if flags.ffmpeg_missing {
        return ExtractError::FfmpegUnavailable;
    }
    if flags.extractor {
        return ExtractError::ExtractionFailure;
    }
    ExtractError::ProcessFailed
}
/// Await to completion and cancel through the watch channel, so direct-child
/// exit is observed. Dropping this future uses kill_on_drop as a last resort;
/// the parent process owner still must reap/contain the entire job/tree.
async fn extract_impl(
    components: &Components,
    video: &VideoId,
    route: &NativeRoute,
    mut cancel: watch::Receiver<bool>,
    failure_fingerprint: &mut FailureFingerprint,
) -> Result<Zeroizing<Vec<u8>>, ExtractError> {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(ExtractError::Cancelled);
    }
    let mut child = command(components, video, route)?
        .spawn()
        .map_err(|_| ExtractError::SpawnFailed)?;
    let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
        (Some(out), Some(err)) => (out, err),
        _ => {
            let _ = child.start_kill();
            return match tokio::time::timeout(REAP_TIMEOUT, child.wait()).await {
                Ok(Ok(_)) => Err(ExtractError::ReadFailed),
                _ => Err(ExtractError::ReapFailed),
            };
        }
    };
    let result = {
        let work = async {
            // Both pipes are drained concurrently. Bounded stderr is retained
            // only in zeroizing memory for fixed-class diagnostics; never logged.
            let (out, diagnostic) = tokio::try_join!(
                bounded(stdout, STDOUT_LIMIT, true),
                bounded(stderr, STDERR_LIMIT, true)
            )?;
            let status = child.wait().await.map_err(|_| ExtractError::WaitFailed)?;
            if !status.success() {
                *failure_fingerprint = fingerprint(&diagnostic, status.code());
                return Err(classify_failure(&diagnostic));
            }
            if out.is_empty() {
                return Err(ExtractError::EmptyOutput);
            }
            Ok(out)
        };
        tokio::select! {
         biased;
         _=cancelled(&mut cancel)=>Err(ExtractError::Cancelled),
         _=tokio::time::sleep(EXECUTION_TIMEOUT)=>Err(ExtractError::TimedOut),
         answer=work=>answer,
        }
    };
    if result.is_err() {
        // start_kill may fail if the process already exited; always wait regardless.
        let _ = child.start_kill();
        match tokio::time::timeout(REAP_TIMEOUT, child.wait()).await {
            Ok(Ok(_)) => {}
            _ => return Err(ExtractError::ReapFailed),
        }
    }
    result
}

/// Existing sanitized API; direct callers do not need diagnostic metadata.
pub async fn extract(
    components: &Components,
    video: &VideoId,
    route: &NativeRoute,
    cancel: watch::Receiver<bool>,
) -> Result<Zeroizing<Vec<u8>>, ExtractError> {
    let mut diagnostic = FailureFingerprint::default();
    extract_impl(components, video, route, cancel, &mut diagnostic).await
}
/// Diagnostic API for controlled native probes. No raw stderr/URLs are exposed.
pub async fn extract_diagnosed(
    components: &Components,
    video: &VideoId,
    route: &NativeRoute,
    cancel: watch::Receiver<bool>,
) -> Result<Zeroizing<Vec<u8>>, ExtractFailure> {
    let mut diagnostic = FailureFingerprint::default();
    extract_impl(components, video, route, cancel, &mut diagnostic)
        .await
        .map_err(|error| ExtractFailure { error, diagnostic })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_errors_are_static_without_process_diagnostics() {
        let errors = [
            ExtractError::InvalidComponents,
            ExtractError::InvalidRoute,
            ExtractError::Cancelled,
            ExtractError::SpawnFailed,
            ExtractError::OutputLimit,
            ExtractError::ReadFailed,
            ExtractError::WaitFailed,
            ExtractError::ProcessFailed,
            ExtractError::UnsupportedInvocation,
            ExtractError::HttpForbidden,
            ExtractError::RateLimited,
            ExtractError::TlsFailure,
            ExtractError::ChallengeRequired,
            ExtractError::VideoUnavailable,
            ExtractError::HttpBadRequest,
            ExtractError::HttpNotFound,
            ExtractError::HttpServerError,
            ExtractError::NetworkFailure,
            ExtractError::RuntimeFailure,
            ExtractError::ChallengeSolverUnavailable,
            ExtractError::FormatUnavailable,
            ExtractError::FfmpegUnavailable,
            ExtractError::ExtractionFailure,
            ExtractError::TimedOut,
            ExtractError::ReapFailed,
            ExtractError::EmptyOutput,
        ];
        for error in errors {
            let display = error.to_string();
            assert!(!display.contains("http"));
            assert!(!display.contains("cookie"));
            assert!(display.len() < 160);
        }
    }
    #[tokio::test]
    async fn output_limits_accept_boundary_and_refuse_one_extra() {
        let bytes = vec![b'x'; 8193];
        let good = bounded(&bytes[..8192], 8192, true).await.unwrap();
        assert_eq!(good.len(), 8192);
        assert_eq!(
            bounded(bytes.as_slice(), 8192, true).await.unwrap_err(),
            ExtractError::OutputLimit
        );
        assert!(bounded(bytes.as_slice(), 8193, false)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            bounded(bytes.as_slice(), 8192, false).await.unwrap_err(),
            ExtractError::OutputLimit
        );
    }
    #[tokio::test]
    async fn owner_channel_drop_cancels_even_without_explicit_true() {
        let (tx, mut rx) = watch::channel(false);
        drop(tx);
        tokio::time::timeout(Duration::from_secs(1), cancelled(&mut rx))
            .await
            .unwrap();
    }
    #[test]
    fn invocation_is_canonical_anon_and_never_disables_tls_verification() {
        let executable = std::env::current_exe().unwrap();
        let components = Components::new(executable.clone(), executable).unwrap();
        let video = VideoId::parse("dQw4w9WgXcQ").unwrap();
        let route = NativeRoute {
            key: "a".repeat(64),
            proxy: "http://127.0.0.1:1234".into(),
            allow_non_proxied_udp: false,
        };
        let command = command(&components, &video, &route).unwrap();
        let args = command
            .as_std()
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            args.last(),
            Some(&"https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
        for flag in [
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-cookies",
            "--no-cookies-from-browser",
            "--no-remote-components",
            "--no-js-runtimes",
            "--skip-download",
        ] {
            assert!(args.contains(&flag));
        }
        for forbidden in [
            "--no-check-certificates",
            "--cookies",
            "--cookies-from-browser",
            "--netrc",
            "--exec",
        ] {
            assert!(!args.contains(&forbidden));
        }
        let proxy = args.iter().position(|a| *a == "--proxy").unwrap();
        assert_eq!(args[proxy + 1], route.proxy);
        let mut bad = route;
        bad.proxy = "socks5://secret@remote:1080".into();
        assert!(super::command(&components, &video, &bad).is_err());
    }
    #[test]
    fn diagnostic_classification_never_reflects_remote_urls_or_secrets() {
        for (text, expected) in [
            (
                "ERROR: no such option: --secret-option",
                ExtractError::UnsupportedInvocation,
            ),
            ("HTTP Error 403: Forbidden", ExtractError::HttpForbidden),
            (
                "HTTP Error 429: Too Many Requests",
                ExtractError::RateLimited,
            ),
            (
                "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed",
                ExtractError::TlsFailure,
            ),
            (
                "Sign in to confirm you're not a bot",
                ExtractError::ChallengeRequired,
            ),
            (
                "Video unavailable. This video has been removed",
                ExtractError::VideoUnavailable,
            ),
            ("opaque upstream failure", ExtractError::ProcessFailed),
        ] {
            let bytes = Zeroizing::new(format!(
                "{text} https://secret.invalid/path?cookie=TOKEN&auth=PASS"
            ));
            let actual = classify_failure(bytes.as_bytes());
            assert_eq!(actual, expected);
            let public = format!("{actual:?} {actual}");
            for secret in ["secret.invalid", "TOKEN", "PASS", "cookie=", "https://"] {
                assert!(!public.contains(secret));
            }
        }
    }
    #[test]
    fn diagnostic_matching_handles_pipe_boundaries_non_utf8_and_specific_precedence() {
        let mut bytes = Zeroizing::new(vec![0xff; 8190]);
        bytes.extend_from_slice(b"HTTP Error 429: Too Many Requests");
        assert_eq!(classify_failure(&bytes), ExtractError::RateLimited);
        assert_eq!(
            classify_failure(b"HTTP Error 403: Forbidden. Sign in to confirm you're not a bot"),
            ExtractError::ChallengeRequired
        );
        assert_eq!(
            classify_failure(b"Deno EJS https://example.invalid/deno/ejs"),
            ExtractError::ProcessFailed
        );
        assert_eq!(classify_failure(b""), ExtractError::ProcessFailed);
    }
    #[test]
    fn richer_diagnostics_cover_official_errors_without_exporting_payloads() {
        for (source, expected) in [
            ("HTTP Error 400: Bad Request", ExtractError::HttpBadRequest),
            ("HTTP Error 404: Not Found", ExtractError::HttpNotFound),
            (
                "HTTP Error 503: Service Unavailable",
                ExtractError::HttpServerError,
            ),
            (
                "Remote end closed connection without response",
                ExtractError::NetworkFailure,
            ),
            (
                "Error running deno process (returncode: 1)",
                ExtractError::RuntimeFailure,
            ),
            (
                "Requested format is not available",
                ExtractError::FormatUnavailable,
            ),
            ("ffmpeg is not installed", ExtractError::FfmpegUnavailable),
            (
                "Unable to extract initial player response",
                ExtractError::ExtractionFailure,
            ),
        ] {
            let bytes = Zeroizing::new(format!(
                "ERROR: {source} https://private.invalid/path?auth=SECRET"
            ));
            assert_eq!(classify_failure(bytes.as_bytes()), expected);
            let diagnostic = fingerprint(bytes.as_bytes(), Some(3));
            let serialized = serde_json::to_string(&diagnostic).unwrap();
            assert_eq!(diagnostic.exit_code, Some(3));
            assert!(diagnostic.error_prefix);
            assert!(!serialized.contains("SECRET"));
            assert!(!serialized.contains("private.invalid"));
        }
    }
}
