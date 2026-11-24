//! Native-only save admission and bounded owned-CEF download workers. The page
//! selects an admitted quality; only the parent native dialog supplies a path.
use crate::{
    tab_routes::NativeRoute,
    youtube_light_download::{self as download, DownloadError, Progress},
    youtube_light_extract::ExtractedVideo,
};
use cef::*;
use rand_core::{OsRng, RngCore};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{mpsc::Sender, Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::watch;
const MAX_JOBS: usize = 2;
struct Entry {
    request: String,
    operation: u64,
    document: String,
    route_generation: u64,
    data: Arc<ExtractedVideo>,
    route: NativeRoute,
    audio: bool,
    format: String,
    created: Instant,
    started: bool,
    cancel: watch::Sender<bool>,
    progress: Option<Progress>,
    result: Option<Result<download::Completed, DownloadError>>,
    latched: bool,
    detached: bool,
}
fn jobs() -> &'static Mutex<HashMap<i32, Entry>> {
    static JOBS: OnceLock<Mutex<HashMap<i32, Entry>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}
fn display(browser: &Browser, data: serde_json::Value) {
    if let Some(frame) = browser.main_frame() {
        frame.execute_java_script(
            Some(&CefString::from(
                format!("window.NB_LIGHT && window.NB_LIGHT.setDownload({data});").as_str(),
            )),
            None,
            0,
        );
    }
}
pub fn any_active() -> bool {
    jobs().lock().map_or(true, |all| !all.is_empty())
}
pub fn cancel(browser: i32) {
    if let Ok(mut all) = jobs().lock() {
        if let Some(entry) = all.get_mut(&browser) {
            entry.detached = true;
            let _ = entry.cancel.send(true);
            if !entry.started {
                all.remove(&browser);
            }
        }
    }
}
pub fn cancel_user(browser: i32) {
    if let Ok(mut all) = jobs().lock() {
        if let Some(entry) = all.get_mut(&browser) {
            if entry.started {
                let _ = entry.cancel.send(true);
            } else {
                entry.result = Some(Err(DownloadError::Cancelled));
            }
        }
    }
}
pub fn request(
    browser: &Browser,
    tab: u64,
    route_generation: u64,
    route: NativeRoute,
    audio: bool,
    format: String,
    sender: &Sender<crate::cef_vkladki::Sobytie>,
) {
    let Some((operation, document, data)) = crate::cef_youtube::media(browser) else {
        return;
    };
    let allowed = if audio {
        data.formats.iter().any(download::supports_audio)
    } else {
        data.formats
            .iter()
            .any(|f| f.format_id == format && download::supports_video(f))
    };
    if !allowed || !route.validate() {
        display(
            browser,
            serde_json::json!({"error":"Формат или путь загрузки недоступен"}),
        );
        return;
    }
    let mut nonce = [0u8; 16];
    if OsRng.try_fill_bytes(&mut nonce).is_err() {
        return;
    }
    let request = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let filename = format!(
        "YouTube-{}-{}.webm",
        data.metadata.id().as_str(),
        if audio { "audio" } else { format.as_str() }
    );
    let identity = browser.identifier();
    let (cancel, _) = watch::channel(false);
    let admitted = {
        let Ok(mut all) = jobs().lock() else {
            return;
        };
        if all.len() >= MAX_JOBS || all.contains_key(&identity) {
            false
        } else {
            all.insert(
                identity,
                Entry {
                    request: request.clone(),
                    operation,
                    document,
                    route_generation,
                    data,
                    route,
                    audio,
                    format,
                    created: Instant::now(),
                    started: false,
                    cancel,
                    progress: None,
                    result: None,
                    latched: false,
                    detached: false,
                },
            );
            true
        }
    };
    if !admitted {
        display(
            browser,
            serde_json::json!({"error":"Загрузка уже выполняется или достигнут лимит двух задач"}),
        );
        return;
    }
    display(browser, serde_json::json!({"choosing":true}));
    if sender.send(crate::cef_vkladki::Sobytie::Sluzhebnoe(serde_json::json!({"tip":"youtube-save","id":tab,"browser_id":browser.identifier(),"route_generation":route_generation,"request":request,"filename":filename}))).is_err(){cancel(browser.identifier());}
}
pub fn target(browser: &Browser, route_generation: u64, request: &str, path: Option<String>) {
    let identity = browser.identifier();
    let admitted = jobs().lock().is_ok_and(|all| {
        all.get(&identity).is_some_and(|e| {
            !e.started && e.request == request && e.route_generation == route_generation
        })
    });
    if !admitted {
        return;
    }
    let Some((operation, document, _)) = crate::cef_youtube::media(browser) else {
        cancel(identity);
        return;
    };
    let Some(path) = path else {
        cancel(identity);
        display(browser, serde_json::json!({"cancelled":true}));
        return;
    };
    if path.len() > 4096 {
        cancel(identity);
        return;
    }
    let work = {
        let Ok(mut all) = jobs().lock() else {
            return;
        };
        let Some(entry) = all.get_mut(&identity).filter(|e| {
            !e.started
                && e.request == request
                && e.operation == operation
                && e.document == document
                && e.route_generation == route_generation
                && e.created.elapsed() < Duration::from_secs(120)
        }) else {
            return;
        };
        // A real receiver must be installed before setting started. The pending
        // dialog sender has intentionally no consumer and is never a receipt.
        let (cancel, rx) = watch::channel(false);
        entry.cancel = cancel;
        entry.started = true;
        (
            entry.data.clone(),
            entry.route.clone(),
            entry.audio,
            entry.format.clone(),
            entry.request.clone(),
            rx,
        )
    };
    let directory = std::env::args().find_map(|a| {
        a.strip_prefix("--nb-youtube-components=")
            .map(PathBuf::from)
    });
    let worker_request = work.4.clone();
    let spawned = std::thread::Builder::new()
        .name("youtube-download".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (data, route, audio, format, _, cancel) = work;
                let path = PathBuf::from(path);
                let directory_path = path
                    .parent()
                    .ok_or(DownloadError::InvalidInput)?
                    .to_path_buf();
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or(DownloadError::InvalidInput)?
                    .to_owned();
                let target = download::Target::new(directory_path, filename)?;
                let mux = if audio {
                    None
                } else {
                    Some(
                        crate::youtube_light_components::load_mux(
                            &directory.ok_or(DownloadError::MuxUnavailable)?,
                        )
                        .map_err(|_| DownloadError::MuxUnavailable)?,
                    )
                };
                let audio_track = data
                    .formats
                    .iter()
                    .filter(|f| download::supports_audio(f))
                    .last()
                    .ok_or(DownloadError::UnsupportedFormat)?;
                let selection = if audio {
                    download::Selection::Audio(audio_track)
                } else {
                    download::Selection::Video {
                        video: data
                            .formats
                            .iter()
                            .find(|f| f.format_id == format)
                            .ok_or(DownloadError::UnsupportedFormat)?,
                        audio: audio_track,
                    }
                };
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| DownloadError::Storage)?;
                runtime.block_on(download::run(
                    selection,
                    &route,
                    &target,
                    mux.as_ref(),
                    download::Limits::default(),
                    cancel,
                    |progress| {
                        let Ok(mut all) = jobs().lock() else {
                            return false;
                        };
                        let Some(entry) = all
                            .get_mut(&identity)
                            .filter(|e| e.request == worker_request)
                        else {
                            return false;
                        };
                        entry.progress = Some(progress);
                        !*entry.cancel.borrow()
                    },
                ))
            }))
            .unwrap_or(Err(DownloadError::CleanupUnknown));
            if let Ok(mut all) = jobs().lock() {
                if let Some(entry) = all
                    .get_mut(&identity)
                    .filter(|e| e.request == worker_request)
                {
                    entry.latched = matches!(&result, Err(DownloadError::CleanupUnknown));
                    entry.result = Some(result);
                }
            }
        });
    if spawned.is_err() {
        if let Ok(mut all) = jobs().lock() {
            if let Some(entry) = all.get_mut(&identity) {
                entry.result = Some(Err(DownloadError::Storage));
            }
        }
    }
}
pub fn poll(browsers: &HashMap<u64, Browser>) {
    let owned = browsers
        .values()
        .map(|b| b.identifier())
        .collect::<std::collections::HashSet<_>>();
    let mut updates = Vec::new();
    {
        let Ok(mut all) = jobs().lock() else {
            return;
        };
        let mut remove = Vec::new();
        for (&identity, entry) in all.iter_mut() {
            if !owned.contains(&identity) && !entry.detached {
                continue;
            }
            if entry.detached {
                entry.progress = None;
                if entry.result.is_some() && !entry.latched {
                    remove.push(identity);
                }
                continue;
            }
            if !entry.started && entry.created.elapsed() > Duration::from_secs(120) {
                updates.push((
                    identity,
                    entry.operation,
                    entry.document.clone(),
                    serde_json::json!({"cancelled":true}),
                ));
                remove.push(identity);
                continue;
            }
            if let Some(result) = entry.result.take() {
                let value = match result {
                    Ok(done) => serde_json::json!({"done":true,"bytes":done.bytes}),
                    Err(DownloadError::Cancelled) => serde_json::json!({"cancelled":true}),
                    Err(error) => serde_json::json!({"error":error.to_string()}),
                };
                updates.push((identity, entry.operation, entry.document.clone(), value));
                if !entry.latched {
                    remove.push(identity);
                }
                continue;
            }
            if let Some(progress) = entry.progress.take() {
                updates.push((
                    identity,
                    entry.operation,
                    entry.document.clone(),
                    serde_json::json!({"progress":progress}),
                ));
            }
        }
        for identity in remove {
            all.remove(&identity);
        }
    }
    for (identity, operation, document, value) in updates {
        if let Some(browser) = browsers.values().find(|b| b.identifier() == identity) {
            if crate::cef_youtube::documents().document_matches(identity, operation, &document)
                && browser
                    .main_frame()
                    .is_some_and(|f| CefString::from(&f.url()).to_string() == document)
            {
                display(browser, value);
            }
        }
    }
}
