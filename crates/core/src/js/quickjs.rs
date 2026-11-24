//! QuickJS through `rquickjs`: the portable fallback. Pure C, builds for every Rust target
//! (including iOS device with no JIT, since QuickJS is an interpreter by design). From the S3
//! spike (`ios/spike-js/src/quickjs.rs`).

use rquickjs::context::EvalOptions;
use rquickjs::{Coerced, Context, Runtime};

use super::inproc::SyncJs;

// rquickjs's Runtime/Context are !Send by default; the context is only ever used under the
// owning `InProcContext`'s mutex, one thread at a time.
unsafe impl Send for Quick {}

pub struct Quick {
    _rt: Runtime,
    ctx: Context,
}

impl Quick {
    pub fn new() -> Self {
        let rt = Runtime::new().expect("quickjs runtime");
        // player.js nests deeply; the 256 KiB default trips "stack overflow" on some players.
        rt.set_max_stack_size(4 * 1024 * 1024);
        let ctx = Context::full(&rt).expect("quickjs context");
        Quick { _rt: rt, ctx }
    }
}

impl Default for Quick {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncJs for Quick {
    fn eval(&mut self, src: &str) -> Result<String, String> {
        // Sloppy mode, like a browser <script>: player.js opens with `var window=this;` inside a
        // plain function call, which is `undefined` under rquickjs's default strict eval.
        let mut opts = EvalOptions::default();
        opts.strict = false;
        self.ctx.with(|ctx| match ctx.eval_with_options::<Coerced<String>, _>(src, opts) {
            Ok(v) => Ok(v.0),
            Err(rquickjs::Error::Exception) => {
                let e = ctx.catch();
                let msg = e
                    .as_exception()
                    .map(|x| format!("{} {}", x.message().unwrap_or_default(), x.stack().unwrap_or_default()))
                    .unwrap_or_else(|| format!("{e:?}"));
                Err(msg)
            }
            Err(e) => Err(e.to_string()),
        })
    }
}
