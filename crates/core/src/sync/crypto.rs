//! End-to-end encryption for sync (docs/SYNC.md §6).
//!
//! - The group key is 32 random bytes made by the first device and handed to each joining device
//!   inside the PAKE-protected pairing channel. It never leaves the devices (Keychain).
//! - Every message is XChaCha20-Poly1305 under `HKDF-SHA256(group key, "spotless-sync v1 enc")`
//!   with a random 24-byte nonce, and the pair id, the sender and the message kind as associated
//!   data, so a box can't be replayed under another sender or kind without failing to open.
//! - The relay authenticates a device with `HKDF(group key, "spotless-sync v1 relay")`, which
//!   proves membership without revealing the encryption key.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

const VERSION: u8 = 1;

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub fn derive(secret: &[u8], info: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(b"spotless-sync"), secret);
    let mut out = [0u8; 32];
    hk.expand(info.as_bytes(), &mut out).expect("32 bytes is a valid HKDF length");
    out
}

/// Constant-time equality for MACs and tokens.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A sealed-box key.
#[derive(Clone)]
pub struct BoxKey([u8; 32]);

impl BoxKey {
    /// The group's message key.
    pub fn for_group(group_key: &[u8]) -> Self {
        Self(derive(group_key, "spotless-sync v1 enc"))
    }

    pub fn raw(key: [u8; 32]) -> Self {
        Self(key)
    }

    /// `version || nonce || ciphertext`, base64url.
    pub fn seal(&self, aad: &str, plain: &[u8]) -> String {
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let nonce: [u8; 24] = random_bytes();
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad: aad.as_bytes() })
            .expect("encryption with a valid key cannot fail");
        let mut out = Vec::with_capacity(1 + 24 + ct.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        B64.encode(out)
    }

    /// `None` for anything tampered with, truncated, sealed under another key or another `aad`.
    pub fn open(&self, aad: &str, sealed: &str) -> Option<Vec<u8>> {
        let raw = B64.decode(sealed).ok()?;
        if raw.len() < 1 + 24 + 16 || raw[0] != VERSION {
            return None;
        }
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        cipher.decrypt(XNonce::from_slice(&raw[1..25]), Payload { msg: &raw[25..], aad: aad.as_bytes() }).ok()
    }
}

/// The relay's bearer token for a group (64 hex chars).
pub fn relay_token(group_key: &[u8]) -> String {
    hex(&derive(group_key, "spotless-sync v1 relay"))
}

/// A short public tag for mDNS so a device finds its own group's peers without publishing the
/// pair id (8 hex chars).
pub fn lan_tag(group_key: &[u8]) -> String {
    hex(&derive(group_key, "spotless-sync v1 lan")[..4])
}

pub fn b64(b: &[u8]) -> String {
    B64.encode(b)
}

pub fn unb64(s: &str) -> Option<Vec<u8>> {
    B64.decode(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper_detection() {
        let k = BoxKey::for_group(&[7u8; 32]);
        let sealed = k.seal("pair|dev|ops", b"hello ops");
        assert_eq!(k.open("pair|dev|ops", &sealed).as_deref(), Some(&b"hello ops"[..]));
        // Another aad (sender or kind swapped) fails.
        assert!(k.open("pair|other|ops", &sealed).is_none());
        // Another key fails.
        assert!(BoxKey::for_group(&[8u8; 32]).open("pair|dev|ops", &sealed).is_none());
        // Every single flipped bit fails.
        let raw = unb64(&sealed).unwrap();
        for i in 0..raw.len() {
            let mut bad = raw.clone();
            bad[i] ^= 0x01;
            assert!(k.open("pair|dev|ops", &b64(&bad)).is_none(), "bit flip at {i} went unnoticed");
        }
        // Truncation fails.
        assert!(k.open("pair|dev|ops", &b64(&raw[..raw.len() - 1])).is_none());
        // Two seals of the same text differ (random nonce).
        assert_ne!(sealed, k.seal("pair|dev|ops", b"hello ops"));
    }

    #[test]
    fn derived_secrets_are_distinct_and_stable() {
        let g = [1u8; 32];
        assert_eq!(relay_token(&g), relay_token(&g));
        assert_eq!(relay_token(&g).len(), 64);
        assert_ne!(relay_token(&g), hex(&derive(&g, "spotless-sync v1 enc")));
        assert_eq!(unhex(&hex(&[0, 1, 254, 255])).unwrap(), vec![0, 1, 254, 255]);
        assert!(same(b"abc", b"abc") && !same(b"abc", b"abd") && !same(b"ab", b"abc"));
    }
}
