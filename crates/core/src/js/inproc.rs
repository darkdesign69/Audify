//! In-process JS engines for the cipher (S3, docs/streams/ios-js.md): JavaScriptCore through its C
//! API on Apple platforms, QuickJS (`quickjs` feature) anywhere. Synchronous, so no `call_async`
//! polling is needed; each context sits behind a mutex (one thread at a time) and runs on the
//! blocking pool, never on an async worker or the UI thread (a player.js load is ~65 ms in JSC).
//!
//! iOS uses [`InProcEngine::jsc`] (`crates/ffi`). The desktop keeps its hidden webview for now;
//! the same engine would let it drop that second WebKit process (ios-js.md, note 8).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::{BoxFuture, Error, JsContext, JsEngine, HARNESS_JS};

/// A synchronous JS engine: evaluate a script in one persistent global scope and hand back the
/// completion value as a string.
pub trait SyncJs: Send {
    fn eval(&mut self, src: &str) -> Result<String, String>;
}

/// The browser globals player.js touches while its IIFE runs, measured in S3 (`prelude.js`).
pub const PRELUDE_JS: &str = include_str!("prelude.js");

type Factory = fn() -> Box<dyn SyncJs>;

/// A [`JsEngine`] over a [`SyncJs`] factory.
pub struct InProcEngine {
    name: &'static str,
    make: Factory,
}

impl InProcEngine {
    /// JavaScriptCore (system framework on iOS and macOS, zero binary size).
    #[cfg(target_vendor = "apple")]
    pub fn jsc() -> Self {
        Self { name: "jsc", make: || Box::new(super::jsc::Jsc::new()) }
    }

    /// QuickJS through rquickjs (portable, ~1 MB).
    #[cfg(feature = "quickjs")]
    pub fn quickjs() -> Self {
        Self { name: "quickjs", make: || Box::new(super::quickjs::Quick::new()) }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl JsEngine for InProcEngine {
    fn create<'a>(&'a self, label: &'a str) -> BoxFuture<'a, Result<Arc<dyn JsContext>, Error>> {
        let make = self.make;
        let label = label.to_owned();
        Box::pin(async move {
            let engine = crate::rt::spawn_blocking(move || -> Result<Box<dyn SyncJs>, Error> {
                let mut e = make();
                // The prelude defines `window`; the harness then adds the globals every engine
                // shares with the desktop's webview document (`_yt_player`, `__slots`, `__jserr`).
                e.eval(PRELUDE_JS).map_err(Error::Build)?;
                e.eval(HARNESS_JS).map_err(Error::Build)?;
                Ok(e)
            })
            .await
            .map_err(|e| Error::Build(e.to_string()))??;
            Ok(Arc::new(InProcContext { label, engine: Arc::new(Mutex::new(Some(engine))) })
                as Arc<dyn JsContext>)
        })
    }
}

/// One live context. Dropping the engine (on [`JsContext::destroy`]) frees it (~50 MB in JSC).
pub struct InProcContext {
    label: String,
    engine: Arc<Mutex<Option<Box<dyn SyncJs>>>>,
}

impl InProcContext {
    fn run(&self, js: &str) -> Result<String, Error> {
        let mut g = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        let e = g.as_mut().ok_or_else(|| Error::Gone(self.label.clone()))?;
        e.eval(js).map_err(Error::Eval)
    }
}

impl JsContext for InProcContext {
    fn exists(&self) -> bool {
        self.engine.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    fn eval(&self, js: &str) -> Result<(), Error> {
        self.run(js).map(|_| ())
    }

    fn eval_json(&self, js: String, timeout: Duration) -> BoxFuture<'_, Result<Value, Error>> {
        let engine = self.engine.clone();
        let label = self.label.clone();
        Box::pin(async move {
            // JSON.stringify of `undefined` is `undefined`, which stringifies as "undefined".
            let src = format!("JSON.stringify(({js}))");
            let task = crate::rt::spawn_blocking(move || {
                let mut g = engine.lock().unwrap_or_else(|e| e.into_inner());
                let e = g.as_mut().ok_or(Error::Gone(label))?;
                e.eval(&src).map_err(Error::Eval)
            });
            let raw = tokio::time::timeout(timeout, task)
                .await
                .map_err(|_| Error::Timeout(timeout))?
                .map_err(|e| Error::Eval(e.to_string()))??;
            if raw.is_empty() || raw == "undefined" || raw == "null" {
                return Ok(Value::Null);
            }
            serde_json::from_str(&raw).map_err(|e| Error::Eval(format!("bad json {raw:?}: {e}")))
        })
    }

    fn destroy(&self) -> Result<(), Error> {
        self.engine.lock().unwrap_or_else(|e| e.into_inner()).take();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_vendor = "apple")]
    #[tokio::test]
    async fn jsc_context_answers_json_and_keeps_globals() {
        let engine = InProcEngine::jsc();
        let ctx = engine.create("test").await.unwrap();
        assert_eq!(ctx.eval_json("window.__harness===1".into(), Duration::from_secs(5)).await.unwrap(), Value::Bool(true));
        ctx.eval("window.f=function(s){return s.split('').reverse().join('')};").unwrap();
        assert_eq!(
            ctx.eval_json("window.f('abc')".into(), Duration::from_secs(5)).await.unwrap(),
            Value::String("cba".into())
        );
        assert_eq!(ctx.eval_json("window.nope".into(), Duration::from_secs(5)).await.unwrap(), Value::Null);
        ctx.destroy().unwrap();
        assert!(!ctx.exists());
    }
}
