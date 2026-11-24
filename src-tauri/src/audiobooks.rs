//! stream: audiobooks — Tauri wrappers over `encore_core::audiobooks::api` (the iOS core calls the
//! same bodies from `crates/ffi/src/invoke.rs`). Typed in `ui/src/lib/audiobooks.ts`.

use std::sync::Arc;

use encore_core::audiobooks::{api, Audiobook, AudiobookPage};
use encore_core::smart::home::Shelf;
use tauri::State;

use crate::state::AppState;

type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub async fn audiobook_search(state: St<'_>, query: String) -> Result<Vec<Audiobook>, String> {
    api::audiobook_search(&state, query).await
}

#[tauri::command]
pub async fn audiobook_home(state: St<'_>) -> Result<Vec<Shelf>, String> {
    Ok(api::audiobook_home(&state).await)
}

#[tauri::command]
pub async fn audiobook_genre(state: St<'_>, genre: String, page: Option<usize>) -> Result<Vec<Audiobook>, String> {
    api::audiobook_genre(&state, genre, page).await
}

#[tauri::command]
pub async fn audiobook_page(state: St<'_>, id: String) -> Result<AudiobookPage, String> {
    api::audiobook_page(&state, id).await
}

#[tauri::command]
pub async fn audiobook_for_chapter(state: St<'_>, id: String) -> Result<Option<AudiobookPage>, String> {
    api::audiobook_for_chapter(&state, id).await
}

#[tauri::command]
pub async fn audiobook_save(state: St<'_>, book: Audiobook, saved: bool) -> Result<bool, String> {
    api::audiobook_save(&state, book, saved)
}

#[tauri::command]
pub async fn audiobook_saved(state: St<'_>) -> Result<Vec<Audiobook>, String> {
    api::audiobook_saved(&state)
}
