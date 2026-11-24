//! The Android app's [`encore_core::Host`]: the core's events go out through Tauri's `emit_str`,
//! its JavaScript engine is QuickJS inside the process (the cipher runs YouTube's player.js there)
//! and the asset-protocol scope lets the page load downloaded covers and files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use encore_core::js::JsEngine;
use encore_core::Host;
use tauri::{AppHandle, Emitter, Manager};

pub struct TauriHost {
    app: AppHandle,
    data_dir: PathBuf,
}

impl TauriHost {
    pub fn new(app: AppHandle, data_dir: PathBuf) -> Self {
        Self { app, data_dir }
    }
}

impl Host for TauriHost {
    fn emit_json(&self, event: &str, json: String) {
        let _ = self.app.emit_str(event, json);
    }

    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn download_dir(&self) -> Option<PathBuf> {
        self.app.path().download_dir().ok()
    }

    fn js_engine(&self) -> Option<Arc<dyn JsEngine>> {
        Some(Arc::new(encore_core::js::InProcEngine::quickjs()))
    }

    fn allow_asset_file(&self, path: &Path) {
        let _ = self.app.asset_protocol_scope().allow_file(path);
    }

    fn allow_asset_dir(&self, path: &Path) {
        let _ = self.app.asset_protocol_scope().allow_directory(path, true);
    }
}
