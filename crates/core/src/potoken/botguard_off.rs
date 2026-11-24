//! Stand-in for `botguard.rs` when the `botguard` feature is off (the iOS build: V8 has no
//! prebuilt iOS archive, docs/research/23-ios-plan.md §3). Spawning a minter fails `Fatal`, which
//! `PoTokenGenerator` already treats as "latch and degrade": no PoToken, so streams come from the
//! clients that need none (VISIONOS first), exactly as on a desktop whose minter broke.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("botguard runtime: {0}")]
    Fatal(String),
    #[error("botguard: {0}")]
    Transient(String),
}

pub struct Minter;

pub struct Bootstrap {
    pub minter: Minter,
    pub session_token: String,
    pub lifetime_secs: u64,
}

impl Minter {
    pub async fn spawn(_user_agent: String, _session_ident: String) -> Result<Bootstrap, Error> {
        Err(Error::Fatal("built without the botguard feature".into()))
    }

    pub async fn mint(&self, _ident: &str) -> Result<String, Error> {
        Err(Error::Fatal("built without the botguard feature".into()))
    }
}
