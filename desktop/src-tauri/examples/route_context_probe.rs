//! Synthetic CEF capability experiment; never used by the product.
use cef::*;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn arg(name: &str) -> Result<String, String> {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|a| a[0] == name)
        .map(|a| a[1].clone())
        .ok_or_else(|| format!("missing {name}"))
}
fn loopback(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}

wrap_app! {
    struct ProbeApp { cli_proxy: String }
    impl App {
        fn on_before_command_line_processing(&self, _kind: Option<&CefString>, command: Option<&mut CommandLine>) {
            if let Some(c) = command {
                for flag in ["disable-gpu", "disable-background-networking", "disable-component-update", "disable-quic", "no-sandbox"] {
                    c.append_switch(Some(&CefString::from(flag)));
                }
                if !self.cli_proxy.is_empty() {
                c.append_switch_with_value(Some(&CefString::from("proxy-bypass-list")), Some(&CefString::from("<-loopback>")));
                    c.append_switch_with_value(Some(&CefString::from("proxy-server")), Some(&CefString::from(self.cli_proxy.as_str())));
                }
            }
        }
    }
}
wrap_request_context_handler! {
    struct ContextReady { count: Arc<AtomicUsize> }
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _context: Option<&mut RequestContext>) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }
}
wrap_load_handler! {
    struct Loaded { count: Arc<AtomicUsize> }
    impl LoadHandler {
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, status: i32) {
            if let Some(f) = frame {
                if f.is_main() != 0 && CefString::from(&f.url()).to_string().starts_with("http://") && status == 200 {
                    self.count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }
}
wrap_life_span_handler! {
    struct Closed { count: Arc<AtomicUsize> }
    impl LifeSpanHandler {
        fn on_before_close(&self, _browser: Option<&mut Browser>) { self.count.fetch_add(1, Ordering::SeqCst); }
    }
}
wrap_client! {
    struct ProbeClient { loaded: Arc<AtomicUsize>, closed: Arc<AtomicUsize> }
    impl Client {
        fn load_handler(&self) -> Option<LoadHandler> { Some(Loaded::new(self.loaded.clone())) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Closed::new(self.closed.clone())) }
    }
}
fn context(path: PathBuf, ready: Arc<AtomicUsize>) -> Result<RequestContext, String> {
    let mut settings = RequestContextSettings::default();
    settings.cache_path = CefString::from(path.to_string_lossy().as_ref());
    request_context_create_context(Some(&settings), Some(&mut ContextReady::new(ready)))
        .ok_or_else(|| "context creation failed".into())
}
fn configure(
    context: &RequestContext,
    label: &str,
    proxy: &str,
    sharing: bool,
) -> Result<(), String> {
    let key = CefString::from("proxy");
    let can = context.can_set_preference(Some(&key)) != 0;
    let mut dict = dictionary_value_create().ok_or("dictionary creation failed")?;
    dict.set_string(
        Some(&CefString::from("mode")),
        Some(&CefString::from("fixed_servers")),
    );
    dict.set_string(
        Some(&CefString::from("server")),
        Some(&CefString::from(proxy)),
    );
    dict.set_string(
        Some(&CefString::from("bypass_list")),
        Some(&CefString::from("<-loopback>")),
    );
    let mut value = value_create().ok_or("value creation failed")?;
    value.set_dictionary(Some(&mut dict));
    let mut error = CefString::default();
    let ok = context.set_preference(Some(&key), Some(&mut value), Some(&mut error)) != 0;
    println!(
        "{}",
        serde_json::json!({"context":label,"can_set_proxy":can,"set_proxy_ok":ok,"sharing":sharing})
    );
    // No preference contents or diagnostic strings are serialized.
    Ok(())
}
fn experiment(
    root: PathBuf,
    scenario: &str,
    a: &str,
    b: &str,
    target: &str,
) -> Result<bool, String> {
    let started = Instant::now();
    let ready = Arc::new(AtomicUsize::new(0));
    let mut ca = context(root.join("A"), ready.clone())?;
    let mut cb = if scenario == "shared" {
        request_context_cef_create_context_shared(
            Some(&mut ca),
            Some(&mut ContextReady::new(ready.clone())),
        )
        .ok_or("shared context creation failed")?
    } else {
        context(root.join("B"), ready.clone())?
    };
    while ready.load(Ordering::SeqCst) < 2 && started.elapsed() < Duration::from_secs(10) {
        do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    if ready.load(Ordering::SeqCst) != 2 {
        return Err("context initialization deadline exceeded".into());
    }
    let sharing = ca.is_sharing_with(Some(&mut cb)) != 0;
    configure(&ca, "A", a, sharing)?;
    configure(&cb, "B", b, sharing)?;
    let loaded = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let mut browsers = Vec::new();
    let mut creation_error = None;
    for (label, path, ctx) in [("A", "first", &mut ca), ("B", "second", &mut cb)] {
        // Linux CEF creates a top-level X11 window when parent_window is 0.
        // set_as_popup is a Windows-only cef-rs convenience method.
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            window_name: CefString::from(label),
            bounds: Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            ..WindowInfo::default()
        };
        #[cfg(target_os = "windows")]
        let info = info.set_as_popup(Default::default(), label);
        let url = format!("{}/{path}?context={label}", target.trim_end_matches('/'));
        match browser_host_create_browser_sync(
            Some(&info),
            Some(&mut ProbeClient::new(loaded.clone(), closed.clone())),
            Some(&CefString::from(url.as_str())),
            Some(&BrowserSettings::default()),
            None,
            Some(ctx),
        ) {
            Some(browser) => browsers.push(browser),
            None => {
                creation_error = Some("browser creation failed".to_string());
                break;
            }
        }
    }
    let mut finished = None;
    while creation_error.is_none() && started.elapsed() < Duration::from_secs(35) {
        do_message_loop_work();
        if root.join("done").is_file() {
            let done = finished.get_or_insert_with(Instant::now);
            if done.elapsed() >= Duration::from_secs(1) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let complete = root.join("done").is_file() && loaded.load(Ordering::SeqCst) >= 2;
    for browser in &browsers {
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
    }
    let end = Instant::now();
    while closed.load(Ordering::SeqCst) < browsers.len() && end.elapsed() < Duration::from_secs(5) {
        do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    let clean = closed.load(Ordering::SeqCst) == browsers.len();
    drop(browsers);
    drop(ca);
    drop(cb);
    if !clean {
        return Err("CEF browsers did not close within deadline".into());
    }
    if let Some(error) = creation_error {
        return Err(error);
    }
    Ok(complete)
}
fn run() -> Result<i32, String> {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    if std::env::args().any(|a| a.starts_with("--type=")) {
        return Ok(execute_process(Some(args.as_main_args()), None, std::ptr::null_mut()).max(0));
    }
    let root = PathBuf::from(arg("--probe-root")?);
    if !root.is_absolute() || root.exists() {
        return Err("probe-root must be an absolute, nonexistent disposable directory".into());
    }
    let scenario = arg("--scenario")?;
    if !["independent", "shared", "cli"].contains(&scenario.as_str()) {
        return Err("invalid scenario".into());
    }
    let a = arg("--proxy-a")?;
    let b = arg("--proxy-b")?;
    let target = arg("--target-base")?;
    if !loopback(&a) || !loopback(&b) || !loopback(&target) {
        return Err("only explicit HTTP loopback fixtures are allowed".into());
    }
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let cef_dir = PathBuf::from(std::env::var("CEF_PATH").map_err(|_| "CEF_PATH required")?);
    let mut settings = Settings::default();
    settings.no_sandbox = 1;
    settings.external_message_pump = 1;
    settings.root_cache_path = CefString::from(root.to_string_lossy().as_ref());
    settings.cache_path = CefString::from(root.join("global").to_string_lossy().as_ref());
    settings.resources_dir_path = CefString::from(cef_dir.to_string_lossy().as_ref());
    settings.locales_dir_path = CefString::from(cef_dir.join("locales").to_string_lossy().as_ref());
    settings.log_severity = LogSeverity::DISABLE;
    let mut app = ProbeApp::new(if scenario == "cli" {
        a.clone()
    } else {
        String::new()
    });
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        return Err("CEF initialize failed".into());
    }
    let result = experiment(root, &scenario, &a, &b, &target);
    // Shutdown is safe only after all browser lifetimes ended. On close failure,
    // process exit is bounded; do not enter an unsafe shutdown with live browsers.
    if !matches!(&result, Err(e) if e == "CEF browsers did not close within deadline") {
        shutdown();
    }
    let complete = result?;
    println!("{}", serde_json::json!({"pages_loaded":complete}));
    Ok(if complete { 0 } else { 2 })
}
fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({"error":error,"pages_loaded":false})
            );
            1
        }
    };
    std::process::exit(code);
}
