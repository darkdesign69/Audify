//! The core's async runtime: one process-wide multi-thread tokio runtime, created on first use.
//!
//! The same shape as `tauri::async_runtime` (which the core used before it was Tauri-free). The
//! desktop hands this runtime to Tauri at startup (`tauri::async_runtime::set`), so both share
//! one pool; the iOS app just uses it.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;

static RT: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("encore-core")
            .build()
            .expect("tokio runtime")
    })
}

/// The runtime's handle (for `tauri::async_runtime::set`).
pub fn handle() -> Handle {
    runtime().handle().clone()
}

pub fn spawn<F>(f: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(f)
}

pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().spawn_blocking(f)
}

/// Run a future to completion from synchronous code that is not on the runtime.
pub fn block_on<F: Future>(f: F) -> F::Output {
    runtime().block_on(f)
}
