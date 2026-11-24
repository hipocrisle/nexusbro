//! Disposable-runner-only native prerequisite, never enables product sleep.
//! Parent retains encrypted transient material over stdin/stdout, not disk.
use crate::{
    cef_cookie_transfer::Pending,
    super_cookie_escrow::{self, CookieSnapshot, EphemeralKey, Scope, Sealed},
};
use cef::*;
use serde::Deserialize;
use std::{
    io::{BufRead, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    key: String,
    scope: Scope,
    sealed: Option<Sealed>,
    url: String,
}
impl Drop for Plan {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}
struct State {
    loaded: AtomicBool,
    closed: AtomicBool,
}
wrap_load_handler! {
 struct ProbeLoad { state:Arc<State> }
 impl LoadHandler {
  fn on_loading_state_change(&self,_browser:Option<&mut Browser>,is_loading:i32,_back:i32,_forward:i32){if is_loading==0{self.state.loaded.store(true,Ordering::Release);}}
 }
}
wrap_life_span_handler! {
 struct ProbeLife { state:Arc<State> }
 impl LifeSpanHandler {
  fn on_before_close(&self,_browser:Option<&mut Browser>){self.state.closed.store(true,Ordering::Release);}
 }
}
wrap_client! {
 struct ProbeClient { state:Arc<State> }
 impl Client {
  fn load_handler(&self)->Option<LoadHandler>{Some(ProbeLoad::new(self.state.clone()))}
  fn life_span_handler(&self)->Option<LifeSpanHandler>{Some(ProbeLife::new(self.state.clone()))}
 }
}
wrap_completion_callback! {
 struct Flushed { done:Arc<AtomicBool> }
 impl CompletionCallback {fn on_complete(&self){self.done.store(true,Ordering::Release);}}
}
fn wait(mut condition: impl FnMut() -> bool) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        anyhow::ensure!(Instant::now() < deadline, "Probe deadline");
        do_message_loop_work();
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
fn reply(mut pending: Pending) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let mut result = None;
    wait(|| {
        result = pending.poll();
        result.is_some()
    })?;
    result
        .unwrap()
        .map_err(|_| anyhow::anyhow!("Probe CDP refused"))
}
fn browser(context: &mut RequestContext, url: &str) -> anyhow::Result<(Browser, Arc<State>)> {
    let state = Arc::new(State {
        loaded: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    });
    let info = WindowInfo {
        runtime_style: RuntimeStyle::ALLOY,
        bounds: Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        },
        ..Default::default()
    };
    #[cfg(target_os = "windows")]
    let info = info.set_as_popup(Default::default(), "Native cookie prerequisite");
    let browser = browser_host_create_browser_sync(
        Some(&info),
        Some(&mut ProbeClient::new(state.clone())),
        Some(&CefString::from(url)),
        Some(&BrowserSettings::default()),
        None,
        Some(context),
    )
    .ok_or_else(|| anyhow::anyhow!("Probe browser refused"))?;
    wait(|| state.loaded.load(Ordering::Acquire))?;
    Ok((browser, state))
}
fn close(browser: Browser, state: Arc<State>) -> anyhow::Result<()> {
    browser
        .host()
        .ok_or_else(|| anyhow::anyhow!("Probe host missing"))?
        .close_browser(0);
    wait(|| state.closed.load(Ordering::Acquire))?;
    drop(browser);
    Ok(())
}
fn stage(name: &'static str) -> anyhow::Result<()> {
    println!("NB-PROBE {}", serde_json::json!({"stage":name}));
    std::io::stdout().flush()?;
    Ok(())
}
/// Called after CEF initialization, before ordinary app browser creation.
/// Success returns only after all browser callbacks, cookie flush and ref drops.
pub fn run(
    root: &std::path::Path,
    bootstrap: crate::tab_routes::RouteBootstrap,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var_os("NEXUSBRO_TEST_DATA").is_some(),
        "Probe requires disposable test runner"
    );
    stage("input")?;
    let mut input = Zeroizing::new(Vec::new());
    let count = std::io::stdin()
        .lock()
        .take((super_cookie_escrow::MAX_BYTES * 2 + 1) as u64)
        .read_until(b'\n', &mut input)?;
    anyhow::ensure!(
        count > 0 && count <= super_cookie_escrow::MAX_BYTES * 2 && input.last() == Some(&b'\n'),
        "Probe frame invalid"
    );
    let plan: Plan = serde_json::from_slice(&input)?;
    let url = reqwest::Url::parse(&plan.url)?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https")
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
            && url.username().is_empty()
            && url.password().is_none(),
        "Probe origin invalid"
    );
    anyhow::ensure!(
        bootstrap.bindings.is_empty() && plan.scope.context_key == bootstrap.inherit.key,
        "Probe scope mismatch"
    );
    let key =
        EphemeralKey::from_base64(&plan.key).map_err(|_| anyhow::anyhow!("Probe key refused"))?;
    let route = bootstrap.inherit.clone();
    let mut registry = crate::cef_route_context::Registry::new(root, bootstrap, false)?;
    stage("context")?;
    let mut context = None;
    let mut context_error = false;
    wait(|| match registry.prepare(&route, false) {
        Ok(value) => {
            context = value;
            context.is_some()
        }
        Err(_) => {
            context_error = true;
            true
        }
    })?;
    anyhow::ensure!(!context_error, "Probe context refused");
    let mut context = context.unwrap();
    // This blank carrier is the only browser before authenticated readback.
    stage("anchor")?;
    let (anchor, anchor_state) = browser(&mut context, "about:blank")?;
    if let Some(sealed) = &plan.sealed {
        stage("open")?;
        let clear = super_cookie_escrow::open(&plan.scope, &key, sealed)
            .map_err(|_| anyhow::anyhow!("Probe envelope refused"))?;
        stage("parse")?;
        let expected = CookieSnapshot::from_cdp_result(&clear)
            .map_err(|_| anyhow::anyhow!("Probe snapshot refused"))?;
        stage("restore")?;
        let _ = reply(
            Pending::restore(&anchor, &expected)
                .map_err(|_| anyhow::anyhow!("Probe restore refused"))?,
        )?;
        stage("readback")?;
        let actual =
            reply(Pending::get(&anchor).map_err(|_| anyhow::anyhow!("Probe readback refused"))?)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        stage("verify")?;
        if expected.verify_readback_at(&actual, now).is_err() {
            println!(
                "NB-PROBE {}",
                serde_json::json!({"diagnostic":expected.readback_diagnostic_at(&actual,now)})
            );
            std::io::stdout().flush()?;
            anyhow::bail!("Probe readback differs");
        }
        println!("NB-PROBE {{\"readback_before_navigation\":true}}");
        std::io::stdout().flush()?;
        stage("admission")?;
        let mut admission = Vec::new();
        std::io::stdin()
            .lock()
            .take(7)
            .read_until(b'\n', &mut admission)?;
        anyhow::ensure!(admission == b"ADMIT\n", "Probe admission refused");
    }
    stage("target")?;
    let (target, target_state) = browser(&mut context, &plan.url)?;
    stage("target_close")?;
    close(target, target_state)?;
    // Origin page is gone before final capture; carrier has no origin script.
    stage("cookie_capture")?;
    let clear =
        reply(Pending::get(&anchor).map_err(|_| anyhow::anyhow!("Probe snapshot unavailable"))?)?;
    let snapshot = CookieSnapshot::from_cdp_result(&clear)
        .map_err(|_| anyhow::anyhow!("Probe cookie schema unsupported"))?;
    stage("seal")?;
    let sealed = super_cookie_escrow::seal(&plan.scope, &key, &snapshot.canonical_bytes())
        .map_err(|_| anyhow::anyhow!("Probe sealing refused"))?;
    stage("anchor_close")?;
    close(anchor, anchor_state)?;
    stage("flush")?;
    let done = Arc::new(AtomicBool::new(false));
    let manager = context
        .cookie_manager(None)
        .ok_or_else(|| anyhow::anyhow!("Probe cookie manager unavailable"))?;
    anyhow::ensure!(
        manager.flush_store(Some(&mut Flushed::new(done.clone()))) != 0,
        "Probe flush refused"
    );
    wait(|| done.load(Ordering::Acquire))?;
    drop(manager);
    drop(context);
    drop(registry);
    stage("complete")?;
    // No clear cookie or key is logged; encrypted frame stays on parent pipe.
    println!(
        "NB-PROBE {}",
        serde_json::json!({"sealed":sealed,"all_browsers_closed":true,"cookie_flush_completed":true})
    );
    std::io::stdout().flush()?;
    Ok(())
}
