//! The JavaScript engine the cipher runs YouTube's `player.js` in (context/13).
//!
//! On the desktop it is a hidden webview (`src-tauri/src/webview.rs`, the `Bridge`); on iOS it is
//! in-process JavaScriptCore through its C API ([`inproc`], from the S3 spike), with QuickJS
//! behind the `quickjs` feature as the portable fallback. The cipher only
//! needs four things from a context: does it still exist, run a script, evaluate a synchronous
//! expression to JSON, and tear it down.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

pub mod inproc;
#[cfg(target_vendor = "apple")]
mod jsc;
#[cfg(feature = "quickjs")]
mod quickjs;

pub use inproc::{InProcEngine, SyncJs};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("webview '{0}' does not exist")]
    Gone(String),
    #[error("webview eval failed: {0}")]
    Eval(String),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    /// Uncaught JS error in the harness → treat the context as bad (context/04 BadWebViewException).
    #[error("webview reported a JS error: {0}")]
    BadWebview(String),
    #[error("build failed: {0}")]
    Build(String),
}

/// Builds fresh JS contexts. The harness document (`_yt_player`, `__slots`, the error hooks) is
/// the engine's job: a context it hands back is ready for `eval`.
pub trait JsEngine: Send + Sync {
    fn create<'a>(&'a self, label: &'a str) -> BoxFuture<'a, Result<Arc<dyn JsContext>, Error>>;
}

/// One live JS context (one hidden webview on the desktop).
pub trait JsContext: Send + Sync {
    /// True while the context still exists (a webview can be closed under us).
    fn exists(&self) -> bool;
    /// Run a script, result ignored.
    fn eval(&self, js: &str) -> Result<(), Error>;
    /// Evaluate a **synchronous** JS expression and return the JSON of its value
    /// (`Value::Null` for null/undefined).
    fn eval_json(&self, js: String, timeout: Duration) -> BoxFuture<'_, Result<Value, Error>>;
    /// Tear it down. Idempotent.
    fn destroy(&self) -> Result<(), Error>;
}

/// The harness every engine loads before handing a context out. `_yt_player` is the IIFE argument
/// YouTube's player.js expects (context/05); the error hooks feed [`Error::BadWebview`], and
/// `__slots` is where async calls park their results. A non-browser engine (JavaScriptCore) needs
/// `window` defined first: `var window = globalThis;`.
pub const HARNESS_JS: &str = "window.__jserr=null;window.__slots={};window._yt_player=window._yt_player||{};\
window.addEventListener&&window.addEventListener('error',function(e){window.__jserr=String((e&&e.message)||e);});\
window.onunhandledrejection=function(e){window.__jserr=String((e.reason&&e.reason.message)||e.reason);};\
window.__harness=1;";

/// Poll a synchronous JS expression until it evaluates non-null, or time out. Also fails fast
/// if the harness recorded an uncaught JS error. An individual eval that times out or errors is
/// treated as "not ready yet" — only the outer `timeout` aborts the poll.
pub async fn poll_json(ctx: &dyn JsContext, expr: &str, timeout: Duration) -> Result<Value, Error> {
    let deadline = std::time::Instant::now() + timeout;
    let step = Duration::from_millis(100);
    loop {
        match ctx.eval_json(format!("({expr})"), Duration::from_secs(3)).await {
            Ok(v) if !v.is_null() => return Ok(v),
            Ok(_) => {}                                        // null → still pending
            Err(Error::Gone(l)) => return Err(Error::Gone(l)), // context died — stop
            Err(_) => {} // a transient eval timeout/error → keep polling until the deadline
        }
        if let Ok(Some(msg)) = ctx
            .eval_json("window.__jserr||null".into(), Duration::from_secs(3))
            .await
            .map(|v| v.as_str().map(str::to_owned))
        {
            return Err(Error::BadWebview(msg));
        }
        if std::time::Instant::now() >= deadline {
            return Err(Error::Timeout(timeout));
        }
        tokio::time::sleep(step).await;
    }
}

/// Run an **async** JS expression: `await (expr)`, capturing the resolved value or error into a
/// slot we then poll. This is how promise-returning code bridges back to Rust.
pub async fn call_async(ctx: &dyn JsContext, expr: &str, timeout: Duration) -> Result<Value, Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let id = N.fetch_add(1, Ordering::Relaxed);
    let kick = format!(
        "(async()=>{{try{{window.__slots['{id}']={{ok:1,v:await ({expr})}};}}\
         catch(e){{window.__slots['{id}']={{ok:0,e:String((e&&e.message)||e)}};}}}})();"
    );
    ctx.eval(&kick)?;
    let res = poll_json(ctx, &format!("window.__slots['{id}']||null"), timeout).await;
    // Free the slot regardless of outcome: an early `?` here used to leave the slot pinned in
    // the harness's heap for the life of the process, once per failed call.
    let _ = ctx.eval(&format!("delete window.__slots['{id}'];"));
    let slot = res?;
    if slot.get("ok").and_then(Value::as_i64) == Some(1) {
        Ok(slot.get("v").cloned().unwrap_or(Value::Null))
    } else {
        Err(Error::Eval(
            slot.get("e").and_then(Value::as_str).unwrap_or("async call failed").to_owned(),
        ))
    }
}
