//! Tauri commands for the local library (likes, saved, playlists, folders, pins, recents). The
//! bodies live in `encore_core::library`, shared with iOS.

use std::collections::HashMap;
use std::sync::Arc;

use encore_core::library::{self, *};
use encore_core::state::AppState;
use innertube::SongItem;
use serde_json::Value;
use tauri::State;

type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub async fn lib_snapshot(state: St<'_>) -> R<LibrarySnapshot> {
    library::lib_snapshot(&state).await
}

#[tauri::command]
pub async fn lib_liked_ids(state: St<'_>) -> R<Vec<String>> {
    library::lib_liked_ids(&state).await
}

#[tauri::command]
pub async fn lib_liked_songs(state: St<'_>) -> R<Vec<LikedEntry>> {
    library::lib_liked_songs(&state).await
}

#[tauri::command]
pub async fn lib_like(state: St<'_>, songs: Vec<SongItem>) -> R<Vec<bool>> {
    library::lib_like(&state, songs).await
}

#[tauri::command]
pub async fn lib_unlike(state: St<'_>, video_ids: Vec<String>) -> R<Vec<LikedEntry>> {
    library::lib_unlike(&state, video_ids).await
}

#[tauri::command]
pub async fn lib_restore_likes(state: St<'_>, entries: Vec<LikedEntry>) -> R<()> {
    library::lib_restore_likes(&state, entries).await
}

#[tauri::command]
pub async fn lib_saved(state: St<'_>, kind: Option<String>) -> R<Vec<SavedItem>> {
    library::lib_saved(&state, kind).await
}

#[tauri::command]
pub async fn lib_save(state: St<'_>, card: Value, synced: Option<bool>) -> R<bool> {
    library::lib_save(&state, card, synced).await
}

#[tauri::command]
pub async fn lib_unsave(state: St<'_>, id: String) -> R<Option<SavedItem>> {
    library::lib_unsave(&state, id).await
}

#[tauri::command]
pub async fn lib_restore_saved(state: St<'_>, item: SavedItem) -> R<()> {
    library::lib_restore_saved(&state, item).await
}

#[tauri::command]
pub async fn lib_mark_synced(state: St<'_>, ids: Vec<String>, synced: Option<bool>) -> R<()> {
    library::lib_mark_synced(&state, ids, synced).await
}

#[tauri::command]
pub async fn lib_playlists(state: St<'_>) -> R<Vec<LibPlaylist>> {
    library::lib_playlists(&state).await
}

#[tauri::command]
pub async fn lib_playlist_create(state: St<'_>, name: Option<String>, description: Option<String>, folder_id: Option<String>) -> R<LibPlaylist> {
    library::lib_playlist_create(&state, name, description, folder_id).await
}

#[tauri::command]
pub async fn lib_playlist_update(state: St<'_>, id: String, name: Option<String>, description: Option<String>) -> R<LibPlaylist> {
    library::lib_playlist_update(&state, id, name, description).await
}

#[tauri::command]
pub async fn lib_playlist_delete(state: St<'_>, id: String) -> R<()> {
    library::lib_playlist_delete(&state, id).await
}

#[tauri::command]
pub async fn lib_playlist_restore(state: St<'_>, id: String) -> R<LibPlaylist> {
    library::lib_playlist_restore(&state, id).await
}

// stream: magic-d4 — Settings › Recover playlists.
#[tauri::command]
pub async fn lib_deleted_playlists(state: St<'_>) -> R<Vec<DeletedPlaylist>> {
    library::lib_deleted_playlists(&state).await
}

#[tauri::command]
pub async fn lib_playlist_entries(state: St<'_>, id: String) -> R<Vec<PlaylistEntry>> {
    library::lib_playlist_entries(&state, id).await
}

#[tauri::command]
pub async fn lib_playlist_duplicates(state: St<'_>, id: String, video_ids: Vec<String>) -> R<Vec<String>> {
    library::lib_playlist_duplicates(&state, id, video_ids).await
}

#[tauri::command]
pub async fn lib_playlist_add(state: St<'_>, id: String, songs: Vec<SongItem>, allow_duplicates: Option<bool>, position: Option<usize>) -> R<AddResult> {
    library::lib_playlist_add(&state, id, songs, allow_duplicates, position).await
}

#[tauri::command]
pub async fn lib_playlist_remove(state: St<'_>, id: String, row_ids: Vec<i64>) -> R<Vec<PlaylistEntry>> {
    library::lib_playlist_remove(&state, id, row_ids).await
}

#[tauri::command]
pub async fn lib_playlist_insert_entries(state: St<'_>, id: String, entries: Vec<PlaylistEntry>) -> R<()> {
    library::lib_playlist_insert_entries(&state, id, entries).await
}

#[tauri::command]
pub async fn lib_playlist_move(state: St<'_>, id: String, row_ids: Vec<i64>, before: Option<i64>) -> R<Vec<i64>> {
    library::lib_playlist_move(&state, id, row_ids, before).await
}

#[tauri::command]
pub async fn lib_playlist_set_order(state: St<'_>, id: String, row_ids: Vec<i64>) -> R<Vec<i64>> {
    library::lib_playlist_set_order(&state, id, row_ids).await
}

#[tauri::command]
pub async fn lib_folders(state: St<'_>) -> R<Vec<Folder>> {
    library::lib_folders(&state).await
}

#[tauri::command]
pub async fn lib_folder_create(state: St<'_>, name: Option<String>, parent_id: Option<String>) -> R<Folder> {
    library::lib_folder_create(&state, name, parent_id).await
}

#[tauri::command]
pub async fn lib_folder_rename(state: St<'_>, id: String, name: String) -> R<String> {
    library::lib_folder_rename(&state, id, name).await
}

#[tauri::command]
pub async fn lib_folder_move(state: St<'_>, id: String, parent_id: Option<String>, index: Option<usize>) -> R<Place> {
    library::lib_folder_move(&state, id, parent_id, index).await
}

#[tauri::command]
pub async fn lib_folder_delete(state: St<'_>, id: String) -> R<FolderSnapshot> {
    library::lib_folder_delete(&state, id).await
}

#[tauri::command]
pub async fn lib_folder_restore(state: St<'_>, snapshot: FolderSnapshot) -> R<Folder> {
    library::lib_folder_restore(&state, snapshot).await
}

#[tauri::command]
pub async fn lib_folder_set_item(state: St<'_>, item_id: String, folder_id: Option<String>, index: Option<usize>) -> R<Option<Place>> {
    library::lib_folder_set_item(&state, item_id, folder_id, index).await
}

#[tauri::command]
pub async fn lib_pins(state: St<'_>) -> R<Vec<Pin>> {
    library::lib_pins(&state).await
}

#[tauri::command]
pub async fn lib_pin(state: St<'_>, item_id: String, kind: Option<String>, index: Option<usize>) -> R<Vec<Pin>> {
    library::lib_pin(&state, item_id, kind, index).await
}

#[tauri::command]
pub async fn lib_unpin(state: St<'_>, item_id: String) -> R<Option<usize>> {
    library::lib_unpin(&state, item_id).await
}

#[tauri::command]
pub async fn lib_pins_reorder(state: St<'_>, item_ids: Vec<String>) -> R<Vec<Pin>> {
    library::lib_pins_reorder(&state, item_ids).await
}

#[tauri::command]
pub async fn lib_note_played(state: St<'_>, item_id: String, kind: Option<String>, card: Option<Value>) -> R<()> {
    library::lib_note_played(&state, item_id, kind, card).await
}

#[tauri::command]
pub async fn lib_recent_contexts(state: St<'_>, limit: Option<usize>) -> R<Vec<RecentContext>> {
    library::lib_recent_contexts(&state, limit).await
}

#[tauri::command]
pub async fn lib_recent_tracks(state: St<'_>, limit: Option<usize>) -> R<Vec<RecentTrack>> {
    library::lib_recent_tracks(&state, limit).await
}

#[tauri::command]
pub async fn lib_last_played(state: St<'_>) -> R<HashMap<String, i64>> {
    library::lib_last_played(&state).await
}

#[tauri::command]
pub async fn lib_hidden_songs(state: St<'_>, context_id: Option<String>) -> R<Vec<HiddenSong>> {
    library::lib_hidden_songs(&state, context_id).await
}

#[tauri::command]
pub async fn lib_hide_song(state: St<'_>, video_id: String, song: Option<SongItem>, context_id: Option<String>) -> R<bool> {
    library::lib_hide_song(&state, video_id, song, context_id).await
}

#[tauri::command]
pub async fn lib_unhide_song(state: St<'_>, video_id: String, context_id: Option<String>) -> R<Option<HiddenSong>> {
    library::lib_unhide_song(&state, video_id, context_id).await
}

#[tauri::command]
pub async fn lib_private_session(state: St<'_>) -> R<PrivateSession> {
    library::lib_private_session(&state).await
}

#[tauri::command]
pub async fn lib_set_private_session(state: St<'_>, enabled: bool) -> R<PrivateSession> {
    library::lib_set_private_session(&state, enabled).await
}

#[tauri::command]
pub async fn lib_migrate_legacy(state: St<'_>, blob: Value) -> R<MigrationReport> {
    library::lib_migrate_legacy(&state, blob).await
}

#[tauri::command]
pub async fn lib_export(state: St<'_>, path: Option<String>) -> R<ExportReport> {
    library::lib_export(&state, path).await
}
