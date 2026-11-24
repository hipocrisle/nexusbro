//! Exact-document captions controller. UI thread only admits a numeric selection;
//! network runs on bounded workers, with the retained native route and no cookies.
use crate::{
    tab_routes::NativeRoute,
    youtube_light_captions::{CaptionTrack, Cue},
};
use cef::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::Sender,
        Arc,
    },
    time::Duration,
};
use tokio::sync::watch;
const MAX_ACTIVE: usize = 4;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
struct Binding {
    operation: u64,
    document: String,
    tracks: Vec<Arc<CaptionTrack>>,
    route: NativeRoute,
    selection: u64,
    cancel: Option<watch::Sender<bool>>,
}
thread_local! {static BINDINGS:RefCell<HashMap<i32,Binding>>=RefCell::new(HashMap::new());}
pub struct Ready {
    browser: i32,
    operation: u64,
    document: String,
    selection: u64,
    cues: Result<Vec<Cue>, &'static str>,
}
pub fn attach(
    browser: i32,
    operation: u64,
    document: String,
    tracks: Vec<Arc<CaptionTrack>>,
    route: NativeRoute,
) {
    closed(browser);
    if tracks.is_empty() || tracks.len() > 256 || !route.validate() {
        return;
    }
    BINDINGS.with(|all| {
        let mut all = all.borrow_mut();
        if all.len() < 128 {
            all.insert(
                browser,
                Binding {
                    operation,
                    document,
                    tracks,
                    route,
                    selection: 0,
                    cancel: None,
                },
            );
        }
    });
}
pub fn closed(browser: i32) {
    BINDINGS.with(|all| {
        if let Some(old) = all.borrow_mut().remove(&browser) {
            if let Some(cancel) = old.cancel {
                let _ = cancel.send(true);
            }
        }
    });
}
pub fn clear() {
    let ids = BINDINGS.with(|all| all.borrow().keys().copied().collect::<Vec<_>>());
    for id in ids {
        closed(id);
    }
}
fn apply(browser: &Browser, data: serde_json::Value) {
    if let Some(frame) = browser.main_frame() {
        frame.execute_java_script(
            Some(&CefString::from(
                format!("window.NB_LIGHT && window.NB_LIGHT.setCaptions({data});").as_str(),
            )),
            None,
            0,
        );
    }
}
/// -1 disables captions. No page-supplied URL, path, route or language is used.
pub fn select(browser: &Browser, index: i32, sender: Sender<crate::cef_vkladki::Sobytie>) {
    if currently_on(ThreadId::UI) == 0 || index < -1 || index > 255 {
        return;
    }
    let actual = browser
        .main_frame()
        .map(|f| CefString::from(&f.url()).to_string())
        .unwrap_or_default();
    let work = BINDINGS.with(|all| {
        let mut all = all.borrow_mut();
        let binding = all.get_mut(&browser.identifier())?;
        if actual != binding.document
            || !crate::cef_youtube::documents().document_matches(
                browser.identifier(),
                binding.operation,
                &actual,
            )
        {
            return None;
        }
        binding.selection = binding.selection.checked_add(1)?;
        if let Some(cancel) = binding.cancel.take() {
            let _ = cancel.send(true);
        }
        if index == -1 {
            return None;
        }
        let track = binding.tracks.get(index as usize)?.clone();
        let (cancel, rx) = watch::channel(false);
        binding.cancel = Some(cancel);
        Some((
            track,
            binding.route.clone(),
            binding.operation,
            binding.document.clone(),
            binding.selection,
            rx,
        ))
    });
    // Clearing is safe only for an admitted exact document, including off.
    if !crate::cef_youtube::documents().document_matches(
        browser.identifier(),
        BINDINGS.with(|all| {
            all.borrow()
                .get(&browser.identifier())
                .map_or(0, |b| b.operation)
        }),
        &actual,
    ) {
        return;
    }
    apply(
        browser,
        serde_json::json!({"cues":[],"loading":work.is_some()}),
    );
    let Some((track, route, operation, document, selection, mut cancel)) = work else {
        return;
    };
    if ACTIVE
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < MAX_ACTIVE).then_some(n + 1)
        })
        .is_err()
    {
        apply(
            browser,
            serde_json::json!({"error":"Субтитры заняты. Выберите язык повторно."}),
        );
        return;
    }
    let identity = browser.identifier();
    let sent = sender.clone();
    let spawned=std::thread::Builder::new().name("youtube-captions".into()).stack_size(1024*1024).spawn(move||{
        struct Slot; impl Drop for Slot {fn drop(&mut self){ACTIVE.fetch_sub(1,Ordering::AcqRel);}}
        let _slot=Slot;
        let result=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"Субтитры недоступны").and_then(|runtime| runtime.block_on(async {
            tokio::select! {
                biased;
                _=cancel.changed()=>Err("Загрузка субтитров отменена"),
                result=tokio::time::timeout(Duration::from_secs(15),crate::youtube_light_captions::fetch(&route,&track))=>match result {Ok(Ok(cues))=>Ok(cues),_=>Err("Не удалось загрузить субтитры")}
            }
        }));
        let _=sent.send(crate::cef_vkladki::Sobytie::YoutubeCaptionReady(Ready{browser:identity,operation,document,selection,cues:result}));
    });
    if spawned.is_err() {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
        apply(browser, serde_json::json!({"error":"Субтитры недоступны"}));
    }
}
pub fn ready(browser: &Browser, ready: Ready) {
    let admitted = BINDINGS.with(|all| {
        all.borrow().get(&ready.browser).is_some_and(|b| {
            b.operation == ready.operation
                && b.selection == ready.selection
                && b.document == ready.document
        })
    });
    if !admitted
        || browser.identifier() != ready.browser
        || !crate::cef_youtube::documents().document_matches(
            ready.browser,
            ready.operation,
            &ready.document,
        )
        || browser
            .main_frame()
            .is_none_or(|f| CefString::from(&f.url()).to_string() != ready.document)
    {
        return;
    }
    apply(
        browser,
        match ready.cues {
            Ok(cues) => serde_json::json!({"cues":cues}),
            Err(error) => serde_json::json!({"error":error}),
        },
    );
}
pub fn browser(ready: &Ready) -> i32 {
    ready.browser
}
