//! ── stream: reliability ── the live WEB_REMIX client version, cached in settings.
//!
//! `clients.json` ships a version that goes stale between releases (it was nine months old when
//! this was written). `sw.js_data`, which the app already fetches for visitorData, carries the
//! current one, so it is refreshed from there at most once a day and applied at startup. See
//! `docs/streams/reliability.md`.

use crate::db::Db;

const VERSION_KEY: &str = "web_remix_version";
const VERSION_AT_KEY: &str = "web_remix_version_at";
/// How long a fetched version is trusted before the next startup re-reads it.
const VERSION_TTL_SECS: u64 = 24 * 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Use the cached live version, if there is one and it is newer than the bundled one.
pub fn apply_cached_version(it: &innertube::InnerTube, db: &Db) {
    let Some(v) = db.get_setting(VERSION_KEY) else { return };
    let clients = innertube::Clients::bundled();
    let bundled = clients.get(innertube::METADATA_CLIENT).map(|c| c.client_version.clone());
    if newer(&v, bundled.as_deref()) {
        it.set_client_version(innertube::METADATA_CLIENT, &v);
    }
}

/// True when the cached version is missing or older than [`VERSION_TTL_SECS`].
pub fn version_is_stale(db: &Db) -> bool {
    let at = db.get_setting(VERSION_AT_KEY).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    now_secs().saturating_sub(at) > VERSION_TTL_SECS
}

/// Remember and apply a freshly fetched version.
pub fn store_version(it: &innertube::InnerTube, db: &Db, version: Option<&str>) {
    let Some(v) = version.filter(|v| innertube::is_version_like(v)) else { return };
    db.set_setting(VERSION_KEY, v);
    db.set_setting(VERSION_AT_KEY, &now_secs().to_string());
    apply_cached_version(it, db);
    tracing::info!(version = v, "WEB_REMIX client version refreshed");
}

/// `a` is a usable version and not older than `b` (versions compare part by part as numbers).
fn newer(a: &str, b: Option<&str>) -> bool {
    if !innertube::is_version_like(a) {
        return false;
    }
    let parts = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    b.is_none_or(|b| parts(a) >= parts(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_newer_well_formed_version_wins() {
        assert!(newer("1.20261110.04.00", Some("1.20260213.01.00")));
        assert!(!newer("1.20250101.01.00", Some("1.20260213.01.00")), "never go backwards");
        assert!(!newer("junk", None));
        assert!(newer("1.20261110.04.00", None));
    }

    #[test]
    fn a_stored_version_is_applied_and_ages_out() {
        let db = crate::smart::test_db();
        let it = innertube::InnerTube::new(Default::default(), None).unwrap();
        let clients = innertube::Clients::bundled();
        let web = clients.get(innertube::METADATA_CLIENT).unwrap();
        assert!(version_is_stale(&db), "nothing stored yet");
        store_version(&it, &db, Some("9.99999999.99.99"));
        assert_eq!(it.client_version(web), "9.99999999.99.99");
        assert!(!version_is_stale(&db));
        db.set_setting(VERSION_AT_KEY, "0");
        assert!(version_is_stale(&db));
        store_version(&it, &db, Some("not a version"));
        assert_eq!(db.get_setting(VERSION_KEY).as_deref(), Some("9.99999999.99.99"));
    }
}
