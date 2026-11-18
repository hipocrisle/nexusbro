// Adapted from NexuSSH, Copyright (c) 2026 hipocrisle, MIT.
//! NexusBro account-sync — Phase-0 client-side end-to-end-encryption primitives.
//!
//! This module is **self-contained crypto** for the account-based sync feature
//! (design: the account-sync design notes). It deliberately does
//! NOT touch Tauri, IO, or the existing `vault.rs`/age stack — it is a separate,
//! simpler symmetric scheme for the sync layer, written as pure functions so it
//! is fully unit-testable and so the (future) sync-server can reproduce the exact
//! same contract.
//!
//! It is wired into the crate ONLY as a module declaration in `lib.rs` so it
//! compiles and is test-covered. No commands, no UI, no app flows use it yet.
//!
//! # Crypto contract (the server + every other device MUST match this exactly)
//!
//! One master password. Split-KDF (Bitwarden model):
//!
//! ```text
//! master_key = Argon2id(password, account_salt, params)          (32 bytes)
//! auth_hash  = HKDF-SHA256(ikm=master_key, salt=account_salt,    (32 bytes)
//!                          info="nexusbro-auth-v1")  -> sent to server as login verifier
//! wrap_key   = HKDF-SHA256(ikm=master_key, salt=[],              (32 bytes)
//!                          info="nexusbro-wrap-v1")  -> stays on device
//! ```
//!
//! The **user key** is a random 32-byte data-encryption key. It is what actually
//! encrypts items, and it is wrapped (AEAD-sealed) under `wrap_key` (the
//! password path) AND under a random `recovery_key` (the emergency-kit path).
//! Because the user key is random and independently wrapped, a password change
//! only re-wraps the (small) user key — it never re-encrypts the data.
//!
//! All AEAD here is **XChaCha20-Poly1305** with a fresh random 24-byte nonce
//! prepended to the ciphertext: `sealed = nonce(24) || ciphertext || tag(16)`.
//!
//! The server only ever stores: `account_salt`, `kdf_params` (json string),
//! `auth_hash` (verifier), `wrapped_user_key`, `recovery_wrapped_user_key`, and
//! per-item ciphertext. It never sees the password, master_key, wrap_key,
//! user_key, or recovery_key.

use argon2::{Algorithm, Argon2, ParamsBuilder, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng, Payload};
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

// ---------------------------------------------------------------------------
// Contract constants — DO NOT change without bumping the version suffix, since
// the server and other devices must derive byte-identical values.
// ---------------------------------------------------------------------------

/// HKDF `info` for the server login verifier (`auth_hash`).
const INFO_AUTH: &[u8] = b"nexusbro-auth-v1";
/// HKDF `info` for the key that wraps the user key under the password.
const INFO_WRAP: &[u8] = b"nexusbro-wrap-v1";

/// Length of the random per-account salt fed to Argon2id (and used as the HKDF
/// salt for `auth_hash`).
pub const ACCOUNT_SALT_LEN: usize = 16;
/// XChaCha20-Poly1305 nonce length.
const NONCE_LEN: usize = 24;
/// All keys in this module are 256-bit.
const KEY_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by the account-crypto layer. No function in this module
/// panics on bad/attacker-controlled input — everything fallible returns this.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// Argon2id key derivation failed (e.g. invalid params for the backend).
    #[error("key derivation failed: {0}")]
    Kdf(String),
    /// HKDF expand step failed (only on absurd output lengths — never for ours).
    #[error("hkdf expand failed")]
    Hkdf,
    /// AEAD seal failed (effectively never for in-memory buffers).
    #[error("encryption failed")]
    Encrypt,
    /// AEAD open failed: wrong key, wrong password, or tampered ciphertext.
    #[error("decryption failed (wrong key/password or corrupted data)")]
    Decrypt,
    /// A sealed blob was shorter than nonce+tag, so it can't be valid.
    #[error("ciphertext too short")]
    Truncated,
    /// `account_salt` had the wrong length.
    #[error("invalid salt length (expected {expected}, got {got})")]
    SaltLen { expected: usize, got: usize },
    /// A field expected to be base64 was not decodable.
    #[error("invalid base64: {0}")]
    Base64(String),
    /// kdf_params json string was malformed.
    #[error("invalid kdf params: {0}")]
    KdfParams(String),
    /// Recovery-key string ("emergency kit") could not be parsed.
    #[error("invalid recovery key: {0}")]
    RecoveryKey(String),
}

type Result<T> = std::result::Result<T, CryptoError>;

// ---------------------------------------------------------------------------
// KDF params (serializable so the server stores them and other devices match)
// ---------------------------------------------------------------------------

/// Argon2id parameters. Stored by the server as a JSON string (`to_string`) and
/// echoed back to clients so every device derives the identical `master_key`.
///
/// Defaults are sane interactive values: 19 MiB memory, 2 iterations, 1 lane.
/// (19 MiB / t=2 / p=1 is the OWASP-recommended Argon2id baseline.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory cost in KiB (`m_cost`). Default 19456 KiB = 19 MiB.
    pub m_cost: u32,
    /// Iteration count (`t_cost`). Default 2.
    pub t_cost: u32,
    /// Degree of parallelism / lanes (`p_cost`). Default 1.
    pub p_cost: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        KdfParams {
            m_cost: 19_456, // 19 MiB
            t_cost: 2,
            p_cost: 1,
        }
    }
}

impl KdfParams {
    /// Serialize to the canonical JSON string the server persists.
    pub fn to_string(&self) -> String {
        // KdfParams is plain numeric fields — serialization cannot fail.
        serde_json::to_string(self).expect("KdfParams serialization is infallible")
    }

    /// Parse from the JSON string form. Inverse of [`KdfParams::to_string`].
    pub fn from_str(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| CryptoError::KdfParams(e.to_string()))
    }

    /// Reject KDF parameters outside safe bounds. These arrive from the SERVER
    /// (prelogin), so an attacker/compromised server could otherwise send an
    /// enormous `m_cost` to exhaust the client's memory (a hang/OOM DoS at login)
    /// — or an absurdly small one to weaken the KDF (offline-cracking downgrade).
    /// The client only ever registers with the defaults (19456/2/1), so these
    /// bounds never reject a legitimate account.
    pub fn validate(&self) -> Result<()> {
        // Browser protocol: 19 MiB .. 64 MiB; 2..4 passes; 1..4 lanes.
        const MIN_M_COST: u32 = 19_456; // 19 MiB
        const MAX_M_COST: u32 = 64 * 1024; // 64 MiB
        let ok = (MIN_M_COST..=MAX_M_COST).contains(&self.m_cost)
            && (2..=4).contains(&self.t_cost)
            && (1..=4).contains(&self.p_cost);
        if ok {
            Ok(())
        } else {
            Err(CryptoError::KdfParams(format!(
                "KDF params out of safe range (m_cost={}, t_cost={}, p_cost={})",
                self.m_cost, self.t_cost, self.p_cost
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// 1. Master key — Argon2id(password, account_salt, params)
// ---------------------------------------------------------------------------

/// Derive the 32-byte master key from the password and per-account salt.
///
/// This is the root of the split-KDF: `auth_hash` and `wrap_key` are both
/// HKDF-derived from this value. The master key never leaves the device.
pub fn derive_master_key(
    password: &str,
    account_salt: &[u8],
    params: &KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    if account_salt.len() != ACCOUNT_SALT_LEN {
        return Err(CryptoError::SaltLen {
            expected: ACCOUNT_SALT_LEN,
            got: account_salt.len(),
        });
    }
    params.validate()?;
    let a2params = ParamsBuilder::new()
        .m_cost(params.m_cost)
        .t_cost(params.t_cost)
        .p_cost(params.p_cost)
        .output_len(KEY_LEN)
        .build()
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, a2params);

    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(password.as_bytes(), account_salt, out.as_mut())
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// 2 & 3. HKDF-derived sub-keys: auth_hash (server verifier) and wrap_key
// ---------------------------------------------------------------------------

/// HKDF-SHA256 expand of `master_key` into a fixed 32-byte output.
fn hkdf32(master_key: &[u8; KEY_LEN], salt: &[u8], info: &[u8]) -> Result<[u8; KEY_LEN]> {
    let hk = Hkdf::<Sha256>::new(Some(salt), master_key);
    let mut out = [0u8; KEY_LEN];
    hk.expand(info, &mut out).map_err(|_| CryptoError::Hkdf)?;
    Ok(out)
}

/// The login verifier sent to the server.
///
/// `HKDF-SHA256(ikm=master_key, salt=account_salt, info="nexusbro-auth-v1")`.
/// The server stores this (it is not the password and reveals nothing about it)
/// and compares it on login. Deterministic for a given password+salt+params.
pub fn auth_hash(master_key: &[u8; KEY_LEN], account_salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    if account_salt.len() != ACCOUNT_SALT_LEN {
        return Err(CryptoError::SaltLen {
            expected: ACCOUNT_SALT_LEN,
            got: account_salt.len(),
        });
    }
    hkdf32(master_key, account_salt, INFO_AUTH)
}

/// The 32-byte key-wrapping key, derived from the master key. Stays on device.
///
/// `HKDF-SHA256(ikm=master_key, salt=[], info="nexusbro-wrap-v1")`.
pub fn wrap_key(master_key: &[u8; KEY_LEN]) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    Ok(Zeroizing::new(hkdf32(master_key, &[], INFO_WRAP)?))
}

// ---------------------------------------------------------------------------
// Low-level AEAD: XChaCha20-Poly1305, nonce(24) || ciphertext || tag(16)
// ---------------------------------------------------------------------------

/// Seal `plaintext` under `key` with a fresh random 24-byte nonce, returning
/// `nonce || ciphertext_with_tag`. `aad` is authenticated but not encrypted —
/// the tag covers it, so opening requires the identical `aad` (used to bind an
/// item's ciphertext to its slot; pass `b""` for slot-independent blobs).
fn aead_seal(key: &[u8; KEY_LEN], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open a `nonce || ciphertext_with_tag` blob under `key` + `aad`. The Poly1305
/// tag is verified in constant time by the AEAD; a wrong key, wrong `aad`, or
/// tampered blob yields [`CryptoError::Decrypt`].
fn aead_open(key: &[u8; KEY_LEN], blob: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    // Need at least nonce + a 16-byte tag to possibly be valid.
    if blob.len() < NONCE_LEN + 16 {
        return Err(CryptoError::Truncated);
    }
    let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
    let nonce = XNonce::from_slice(nonce_bytes);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(nonce, Payload { msg: ct, aad })
        .map_err(|_| CryptoError::Decrypt)
}

// ---------------------------------------------------------------------------
// 4. User key: random DEK, wrapped under a 32-byte key (wrap_key or recovery)
// ---------------------------------------------------------------------------

/// Generate a fresh random 32-byte user key (the data-encryption key) via the
/// OS CSPRNG.
pub fn generate_user_key() -> Zeroizing<[u8; KEY_LEN]> {
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    OsRng.fill_bytes(k.as_mut());
    k
}

/// Wrap (AEAD-seal) the user key under a 32-byte wrapping key (either the
/// password-derived `wrap_key` or a `recovery_key`).
pub fn wrap_user_key(user_key: &[u8; KEY_LEN], wrapping_key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    aead_seal(wrapping_key, user_key, b"") // slot-independent → no AAD
}

/// Unwrap the user key. Returns [`CryptoError::Decrypt`] if the wrapping key is
/// wrong (wrong password / wrong recovery key) or the blob was tampered.
pub fn unwrap_user_key(
    wrapped: &[u8],
    wrapping_key: &[u8; KEY_LEN],
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut pt = aead_open(wrapping_key, wrapped, b"")?;
    if pt.len() != KEY_LEN {
        pt.zeroize();
        return Err(CryptoError::Decrypt);
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&pt);
    pt.zeroize();
    Ok(key)
}

// ---------------------------------------------------------------------------
// 5. Item encryption with the user key
// ---------------------------------------------------------------------------

/// Encrypt an item's plaintext under the user key, binding it to its storage
/// slot via `aad` (the item's canonical key, e.g. `host.<id>`). Output is opaque
/// to the server: `nonce || ciphertext || tag`. Because the tag covers `aad`,
/// the server cannot relocate this ciphertext into a different item's slot and
/// have it decrypt (audit F20).
pub fn encrypt_item(plaintext: &[u8], user_key: &[u8; KEY_LEN], aad: &[u8]) -> Result<Vec<u8>> {
    aead_seal(user_key, plaintext, aad)
}

/// Decrypt an item produced by [`encrypt_item`], checking it was sealed for this
/// slot (`aad`). Falls back to opening WITHOUT aad so items written by older
/// clients (pre-F20, no binding) still decrypt — they get re-sealed with aad on
/// the next push. A slot mismatch on an aad-bound item fails (relocation blocked).
pub fn decrypt_item(ciphertext: &[u8], user_key: &[u8; KEY_LEN], aad: &[u8]) -> Result<Vec<u8>> {
    match aead_open(user_key, ciphertext, aad) {
        Ok(pt) => Ok(pt),
        Err(_) if !aad.is_empty() => aead_open(user_key, ciphertext, b""), // legacy, unbound
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// 6. Recovery key ("emergency kit")
// ---------------------------------------------------------------------------

/// Generate a fresh random 32-byte recovery key. The user key is also wrapped
/// under this (an alternate unwrap path), and the plaintext recovery key is
/// shown to the user once as an emergency kit.
pub fn generate_recovery_key() -> Zeroizing<[u8; KEY_LEN]> {
    generate_user_key()
}

/// RFC 4648 base32 alphabet (no padding), used for the human-readable kit.
const B32: base32::Alphabet = base32::Alphabet::Rfc4648 { padding: false };
/// How many base32 chars per dash-separated group in the printed kit.
const KIT_GROUP: usize = 5;

/// Format a recovery key as a human-readable emergency-kit string:
/// uppercase base32, split into dash-separated groups of 5
/// (e.g. `ABCDE-FGHIJ-...`). 32 bytes -> 52 base32 chars -> 11 groups.
pub fn format_recovery_key(recovery_key: &[u8; KEY_LEN]) -> String {
    let raw = base32::encode(B32, recovery_key); // already uppercase
    raw.as_bytes()
        .chunks(KIT_GROUP)
        .map(|c| std::str::from_utf8(c).expect("base32 output is ASCII"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse an emergency-kit string back into a recovery key. Tolerant of dashes,
/// surrounding whitespace, and lower/upper case; rejects anything that does not
/// decode to exactly 32 bytes.
pub fn parse_recovery_key(kit: &str) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let cleaned: String = kit
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let bytes = base32::decode(B32, &cleaned)
        .ok_or_else(|| CryptoError::RecoveryKey("not valid base32".into()))?;
    if bytes.len() != KEY_LEN {
        let mut b = bytes;
        b.zeroize();
        return Err(CryptoError::RecoveryKey(format!(
            "expected {} bytes, got a different length",
            KEY_LEN
        )));
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&bytes);
    let mut b = bytes;
    b.zeroize();
    Ok(key)
}

// ---------------------------------------------------------------------------
// 7. High-level convenience: registration + login
// ---------------------------------------------------------------------------

/// Everything the client sends to `POST /v1/register`. All binary fields are
/// base64 (STANDARD) so the payload is plain JSON. The server stores these
/// opaquely; it never learns the password or any key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationPayload {
    /// Random 16-byte per-account salt (base64). Fed to Argon2id + HKDF(auth).
    pub account_salt: String,
    /// Argon2id parameters as a JSON string (see [`KdfParams::to_string`]).
    pub kdf_params: String,
    /// Login verifier `auth_hash` (base64).
    pub auth_hash: String,
    /// User key wrapped under the password-derived wrap key (base64).
    pub wrapped_user_key: String,
    /// User key wrapped under the recovery key (base64).
    pub recovery_wrapped_user_key: String,
    /// Login verifier derived from the RECOVERY key (base64) — mirror of
    /// `auth_hash`, computed as `auth_hash(recovery_key, account_salt)`. Lets the
    /// server authenticate a no-password recovery-login.
    pub recovery_auth_hash: String,
}

/// Result of [`prepare_registration`]: the server payload plus the secrets the
/// client keeps in memory after registering.
pub struct Registration {
    /// What to send to `/v1/register`.
    pub payload: RegistrationPayload,
    /// The plaintext recovery key — show to the user ONCE as the emergency kit,
    /// then drop. Use [`format_recovery_key`] to render it.
    pub recovery_key: Zeroizing<[u8; KEY_LEN]>,
    /// The unwrapped user key, ready to encrypt/decrypt items this session.
    pub user_key: Zeroizing<[u8; KEY_LEN]>,
}

/// Build a complete registration from just the master password.
///
/// Generates a random account salt, derives the split-KDF keys, mints a random
/// user key, wraps it under both the password path and a fresh recovery key, and
/// returns both the server payload and the in-memory secrets.
pub fn prepare_registration(password: &str) -> Result<Registration> {
    prepare_registration_with_params(password, &KdfParams::default())
}

/// Like [`prepare_registration`] but with explicit KDF params (used by tests and
/// for tuning; production uses the default).
pub fn prepare_registration_with_params(
    password: &str,
    params: &KdfParams,
) -> Result<Registration> {
    let mut account_salt = [0u8; ACCOUNT_SALT_LEN];
    OsRng.fill_bytes(&mut account_salt);

    let master_key = derive_master_key(password, &account_salt, params)?;
    let ah = auth_hash(&master_key, &account_salt)?;
    let wk = wrap_key(&master_key)?;

    let user_key = generate_user_key();
    let recovery_key = generate_recovery_key();

    let wrapped_user_key = wrap_user_key(&user_key, &wk)?;
    let recovery_wrapped_user_key = wrap_user_key(&user_key, &recovery_key)?;
    // Verifier for the recovery key: same construction as the password auth_hash,
    // but the recovery key plays the role of the master key.
    let recovery_ah = auth_hash(&recovery_key, &account_salt)?;

    let payload = RegistrationPayload {
        account_salt: B64.encode(account_salt),
        kdf_params: params.to_string(),
        auth_hash: B64.encode(ah),
        wrapped_user_key: B64.encode(&wrapped_user_key),
        recovery_wrapped_user_key: B64.encode(&recovery_wrapped_user_key),
        recovery_auth_hash: B64.encode(recovery_ah),
    };

    Ok(Registration {
        payload,
        recovery_key,
        user_key,
    })
}

/// Result of [`login`]: the verifier to send and the unwrapped user key.
pub struct LoginResult {
    /// `auth_hash` (base64) to send to `POST /v1/login` for verification.
    pub auth_hash: String,
    /// The unwrapped user key, ready to decrypt items this session.
    pub user_key: Zeroizing<[u8; KEY_LEN]>,
}

/// Client login: given the password and the per-account material the server
/// returns at the start of login (`account_salt`, `kdf_params`,
/// `wrapped_user_key`, all base64/json), recompute the master key, the
/// `auth_hash` to send for verification, and unwrap the user key.
///
/// A wrong password produces [`CryptoError::Decrypt`] when unwrapping the user
/// key (the recomputed wrap key won't open the blob).
pub fn login(
    password: &str,
    account_salt_b64: &str,
    kdf_params_json: &str,
    wrapped_user_key_b64: &str,
) -> Result<LoginResult> {
    let account_salt = B64
        .decode(account_salt_b64)
        .map_err(|e| CryptoError::Base64(e.to_string()))?;
    let params = KdfParams::from_str(kdf_params_json)?;
    let wrapped = B64
        .decode(wrapped_user_key_b64)
        .map_err(|e| CryptoError::Base64(e.to_string()))?;

    let master_key = derive_master_key(password, &account_salt, &params)?;
    let ah = auth_hash(&master_key, &account_salt)?;
    let wk = wrap_key(&master_key)?;
    let user_key = unwrap_user_key(&wrapped, &wk)?;

    Ok(LoginResult {
        auth_hash: B64.encode(ah),
        user_key,
    })
}

/// Recovery-path unwrap: given the recovery emergency-kit string and the
/// server's `recovery_wrapped_user_key` (base64), recover the user key. Used to
/// reset the master password without losing data.
pub fn recover_user_key(
    kit: &str,
    recovery_wrapped_user_key_b64: &str,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let recovery_key = parse_recovery_key(kit)?;
    let wrapped = B64
        .decode(recovery_wrapped_user_key_b64)
        .map_err(|e| CryptoError::Base64(e.to_string()))?;
    unwrap_user_key(&wrapped, &recovery_key)
}

/// Compute the recovery-login verifier from the emergency-kit string + the
/// account salt — mirror of the password `auth_hash`, used to authenticate a
/// no-password recovery-login.
pub fn recovery_auth_hash_from_kit(kit: &str, account_salt_b64: &str) -> Result<String> {
    let recovery_key = parse_recovery_key(kit)?;
    let account_salt = B64
        .decode(account_salt_b64)
        .map_err(|e| CryptoError::Base64(e.to_string()))?;
    Ok(B64.encode(auth_hash(&recovery_key, &account_salt)?))
}

/// Result of [`rekey_password`]: the new password verifier + the user key
/// re-wrapped under the new password. The account salt is intentionally REUSED
/// (not rotated) so the recovery-key verifier — which is salted with it — stays
/// valid without needing the recovery key during a password change.
pub struct ReKey {
    pub auth_hash: String,
    pub wrapped_user_key: String,
}

/// Re-wrap an already-unwrapped `user_key` under a NEW password, keeping the
/// existing account salt + KDF params. Used by both change-password (have the
/// user key from the live session) and recovery-finish (have it from the
/// recovery-key unwrap).
pub fn rekey_password(
    user_key: &[u8; KEY_LEN],
    new_password: &str,
    account_salt_b64: &str,
    kdf_params_json: &str,
) -> Result<ReKey> {
    let account_salt = B64
        .decode(account_salt_b64)
        .map_err(|e| CryptoError::Base64(e.to_string()))?;
    let params = KdfParams::from_str(kdf_params_json)?;
    let master_key = derive_master_key(new_password, &account_salt, &params)?;
    let ah = auth_hash(&master_key, &account_salt)?;
    let wk = wrap_key(&master_key)?;
    let wrapped = wrap_user_key(user_key, &wk)?;
    Ok(ReKey {
        auth_hash: B64.encode(ah),
        wrapped_user_key: B64.encode(&wrapped),
    })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Cheap KDF params so the test suite stays fast; the crypto contract is
    // identical regardless of cost, only the work factor differs.
    fn fast_params() -> KdfParams {
        KdfParams {
            m_cost: 19_456, // production bounds are tested too
            t_cost: 2,
            p_cost: 1,
        }
    }

    fn fixed_salt() -> [u8; ACCOUNT_SALT_LEN] {
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    }

    // --- KdfParams string round-trip ---------------------------------------

    #[test]
    fn kdf_params_default_values() {
        let d = KdfParams::default();
        assert_eq!(d.m_cost, 19_456);
        assert_eq!(d.t_cost, 2);
        assert_eq!(d.p_cost, 1);
    }

    #[test]
    fn kdf_params_validate_bounds() {
        // Defaults (what the client always registers with) are accepted.
        assert!(KdfParams::default().validate().is_ok());
        // A malicious server sending an enormous m_cost is rejected before we
        // ever allocate (DoS guard), not clamped-and-run.
        assert!(KdfParams {
            m_cost: u32::MAX,
            t_cost: 2,
            p_cost: 1
        }
        .validate()
        .is_err());
        assert!(KdfParams {
            m_cost: 4_000_000,
            t_cost: 2,
            p_cost: 1
        }
        .validate()
        .is_err());
        // Downgrade-weak params are rejected too.
        assert!(KdfParams {
            m_cost: 64,
            t_cost: 1,
            p_cost: 1
        }
        .validate()
        .is_err());
        assert!(KdfParams {
            m_cost: 19_456,
            t_cost: 0,
            p_cost: 1
        }
        .validate()
        .is_err());
        assert!(KdfParams {
            m_cost: 19_456,
            t_cost: 99,
            p_cost: 1
        }
        .validate()
        .is_err());
        assert!(KdfParams {
            m_cost: 19_456,
            t_cost: 2,
            p_cost: 999
        }
        .validate()
        .is_err());
    }

    #[test]
    fn kdf_params_string_round_trip() {
        let p = KdfParams {
            m_cost: 19_456,
            t_cost: 3,
            p_cost: 2,
        };
        let s = p.to_string();
        let back = KdfParams::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn kdf_params_from_bad_string_errors() {
        assert!(matches!(
            KdfParams::from_str("not json"),
            Err(CryptoError::KdfParams(_))
        ));
    }

    // --- derive_master_key -------------------------------------------------

    #[test]
    fn master_key_is_deterministic() {
        let salt = fixed_salt();
        let p = fast_params();
        let a = derive_master_key("hunter2", &salt, &p).unwrap();
        let b = derive_master_key("hunter2", &salt, &p).unwrap();
        assert_eq!(*a, *b);
    }

    #[test]
    fn master_key_differs_for_different_password() {
        let salt = fixed_salt();
        let p = fast_params();
        let a = derive_master_key("hunter2", &salt, &p).unwrap();
        let b = derive_master_key("hunter3", &salt, &p).unwrap();
        assert_ne!(*a, *b);
    }

    #[test]
    fn master_key_differs_for_different_salt() {
        let p = fast_params();
        let mut salt2 = fixed_salt();
        salt2[0] ^= 0xFF;
        let a = derive_master_key("hunter2", &fixed_salt(), &p).unwrap();
        let b = derive_master_key("hunter2", &salt2, &p).unwrap();
        assert_ne!(*a, *b);
    }

    #[test]
    fn master_key_rejects_bad_salt_length() {
        let p = fast_params();
        let err = derive_master_key("hunter2", &[0u8; 8], &p).unwrap_err();
        assert!(matches!(err, CryptoError::SaltLen { .. }));
    }

    // --- auth_hash ---------------------------------------------------------

    #[test]
    fn auth_hash_deterministic_same_password_and_salt() {
        let salt = fixed_salt();
        let p = fast_params();
        let mk1 = derive_master_key("pw", &salt, &p).unwrap();
        let mk2 = derive_master_key("pw", &salt, &p).unwrap();
        assert_eq!(
            auth_hash(&mk1, &salt).unwrap(),
            auth_hash(&mk2, &salt).unwrap()
        );
    }

    #[test]
    fn auth_hash_differs_for_different_password() {
        let salt = fixed_salt();
        let p = fast_params();
        let mk_a = derive_master_key("alpha", &salt, &p).unwrap();
        let mk_b = derive_master_key("bravo", &salt, &p).unwrap();
        assert_ne!(
            auth_hash(&mk_a, &salt).unwrap(),
            auth_hash(&mk_b, &salt).unwrap()
        );
    }

    #[test]
    fn auth_hash_differs_from_master_key_and_wrap_key() {
        // The three split-KDF outputs must be distinct domains.
        let salt = fixed_salt();
        let mk = derive_master_key("pw", &salt, &fast_params()).unwrap();
        let ah = auth_hash(&mk, &salt).unwrap();
        let wk = wrap_key(&mk).unwrap();
        assert_ne!(ah, *mk);
        assert_ne!(ah, *wk);
        assert_ne!(*wk, *mk);
    }

    #[test]
    fn auth_hash_rejects_bad_salt_length() {
        let mk = [0u8; KEY_LEN];
        assert!(matches!(
            auth_hash(&mk, &[0u8; 4]),
            Err(CryptoError::SaltLen { .. })
        ));
    }

    // --- wrap_key ----------------------------------------------------------

    #[test]
    fn wrap_key_is_deterministic() {
        let mk = derive_master_key("pw", &fixed_salt(), &fast_params()).unwrap();
        assert_eq!(*wrap_key(&mk).unwrap(), *wrap_key(&mk).unwrap());
    }

    // --- user key wrap/unwrap round-trip -----------------------------------

    #[test]
    fn user_key_wrap_unwrap_round_trip() {
        let uk = generate_user_key();
        let wk = [7u8; KEY_LEN];
        let wrapped = wrap_user_key(&uk, &wk).unwrap();
        let back = unwrap_user_key(&wrapped, &wk).unwrap();
        assert_eq!(*uk, *back);
    }

    #[test]
    fn user_key_wrap_includes_nonce_prefix_and_tag() {
        let uk = generate_user_key();
        let wk = [7u8; KEY_LEN];
        let wrapped = wrap_user_key(&uk, &wk).unwrap();
        // nonce(24) + 32 plaintext + 16 tag = 72
        assert_eq!(wrapped.len(), NONCE_LEN + KEY_LEN + 16);
    }

    #[test]
    fn user_key_wrap_uses_fresh_nonce_each_time() {
        let uk = generate_user_key();
        let wk = [7u8; KEY_LEN];
        let a = wrap_user_key(&uk, &wk).unwrap();
        let b = wrap_user_key(&uk, &wk).unwrap();
        assert_ne!(
            a, b,
            "two seals of the same data must differ (random nonce)"
        );
    }

    #[test]
    fn unwrap_with_wrong_key_fails_cleanly() {
        let uk = generate_user_key();
        let wk = [7u8; KEY_LEN];
        let wrapped = wrap_user_key(&uk, &wk).unwrap();
        let wrong = [8u8; KEY_LEN];
        assert_eq!(
            unwrap_user_key(&wrapped, &wrong).unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn unwrap_tampered_blob_fails_cleanly() {
        let uk = generate_user_key();
        let wk = [7u8; KEY_LEN];
        let mut wrapped = wrap_user_key(&uk, &wk).unwrap();
        let last = wrapped.len() - 1;
        wrapped[last] ^= 0x01; // flip a tag bit
        assert_eq!(
            unwrap_user_key(&wrapped, &wk).unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn unwrap_truncated_blob_errors() {
        let wk = [7u8; KEY_LEN];
        assert_eq!(
            unwrap_user_key(&[0u8; 10], &wk).unwrap_err(),
            CryptoError::Truncated
        );
    }

    // --- item encrypt/decrypt ----------------------------------------------

    #[test]
    fn item_encrypt_decrypt_round_trip() {
        let uk = generate_user_key();
        let pt = b"ssh host: 10.0.0.1 user=root password=swordfish";
        let ct = encrypt_item(pt, &uk, b"host.a").unwrap();
        let back = decrypt_item(&ct, &uk, b"host.a").unwrap();
        assert_eq!(back, pt);
    }

    #[test]
    fn item_encrypt_empty_plaintext_round_trip() {
        let uk = generate_user_key();
        let ct = encrypt_item(b"", &uk, b"host.a").unwrap();
        assert_eq!(decrypt_item(&ct, &uk, b"host.a").unwrap(), b"");
    }

    #[test]
    fn item_ciphertext_differs_each_time() {
        let uk = generate_user_key();
        let pt = b"same plaintext";
        assert_ne!(
            encrypt_item(pt, &uk, b"host.a").unwrap(),
            encrypt_item(pt, &uk, b"host.a").unwrap()
        );
    }

    #[test]
    fn item_decrypt_with_wrong_user_key_fails() {
        let uk = generate_user_key();
        let other = generate_user_key();
        let ct = encrypt_item(b"secret", &uk, b"host.a").unwrap();
        assert_eq!(
            decrypt_item(&ct, &other, b"host.a").unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn item_decrypt_tampered_fails() {
        let uk = generate_user_key();
        let mut ct = encrypt_item(b"secret payload", &uk, b"host.a").unwrap();
        let mid = ct.len() / 2;
        ct[mid] ^= 0xFF;
        assert_eq!(
            decrypt_item(&ct, &uk, b"host.a").unwrap_err(),
            CryptoError::Decrypt
        );
    }

    // --- F20: AAD binds an item to its slot (anti-relocation) ---------------

    #[test]
    fn item_relocation_to_another_slot_is_rejected() {
        // A malicious server moves host.A's ciphertext into host.B's slot; the
        // client opens host.B with aad="host.b" → must fail (no legacy fallback
        // saves it, because this ciphertext WAS sealed with an aad).
        let uk = generate_user_key();
        let ct = encrypt_item(b"host A password", &uk, b"host.a").unwrap();
        assert_eq!(
            decrypt_item(&ct, &uk, b"host.b").unwrap_err(),
            CryptoError::Decrypt
        );
        // ...but the correct slot still opens.
        assert_eq!(
            decrypt_item(&ct, &uk, b"host.a").unwrap(),
            b"host A password"
        );
    }

    #[test]
    fn item_legacy_unbound_ciphertext_still_opens() {
        // Ciphertext written by a pre-F20 client (sealed with NO aad) must still
        // decrypt via the fallback, whatever slot aad we ask with — so upgrading
        // never loses access to already-synced data (it re-seals on next push).
        let uk = generate_user_key();
        let legacy = aead_seal(&uk, b"legacy secret", b"").unwrap();
        assert_eq!(
            decrypt_item(&legacy, &uk, b"host.a").unwrap(),
            b"legacy secret"
        );
    }

    // --- recovery key ------------------------------------------------------

    #[test]
    fn recovery_key_wraps_and_unwraps_user_key() {
        let uk = generate_user_key();
        let rk = generate_recovery_key();
        let wrapped = wrap_user_key(&uk, &rk).unwrap();
        let back = unwrap_user_key(&wrapped, &rk).unwrap();
        assert_eq!(*uk, *back);
    }

    #[test]
    fn recovery_key_format_parse_round_trip() {
        let rk = generate_recovery_key();
        let kit = format_recovery_key(&rk);
        let parsed = parse_recovery_key(&kit).unwrap();
        assert_eq!(*rk, *parsed);
    }

    #[test]
    fn recovery_kit_format_is_grouped_base32() {
        let rk = [0xABu8; KEY_LEN];
        let kit = format_recovery_key(&rk);
        // 32 bytes -> 52 base32 chars -> groups of 5 => 10 full + 1 of 2 = 11 groups.
        let groups: Vec<&str> = kit.split('-').collect();
        assert_eq!(groups.len(), 11);
        for (i, g) in groups.iter().enumerate() {
            if i < 10 {
                assert_eq!(g.len(), KIT_GROUP);
            }
            assert!(g
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
        }
    }

    #[test]
    fn recovery_key_parse_tolerates_whitespace_and_lowercase() {
        let rk = generate_recovery_key();
        let kit = format_recovery_key(&rk);
        let messy = format!("  {}  ", kit.to_lowercase());
        let parsed = parse_recovery_key(&messy).unwrap();
        assert_eq!(*rk, *parsed);
    }

    #[test]
    fn recovery_key_parse_rejects_garbage() {
        // '1', '8', '0' are not in the RFC4648 base32 alphabet -> decode fails.
        assert!(matches!(
            parse_recovery_key("1810-1810"),
            Err(CryptoError::RecoveryKey(_))
        ));
    }

    #[test]
    fn recovery_key_parse_rejects_wrong_length() {
        // Valid base32 but decodes to fewer than 32 bytes.
        assert!(matches!(
            parse_recovery_key("AAAAAAAA"),
            Err(CryptoError::RecoveryKey(_))
        ));
    }

    // --- registration + login flow -----------------------------------------

    #[test]
    fn registration_produces_consistent_payload() {
        let reg = prepare_registration_with_params("master-pw", &fast_params()).unwrap();
        // salt decodes to 16 bytes
        let salt = B64.decode(&reg.payload.account_salt).unwrap();
        assert_eq!(salt.len(), ACCOUNT_SALT_LEN);
        // kdf_params round-trips
        let params = KdfParams::from_str(&reg.payload.kdf_params).unwrap();
        assert_eq!(params, fast_params());
        // auth_hash recomputes from the same password+salt
        let mk = derive_master_key("master-pw", &salt, &params).unwrap();
        let recomputed = B64.encode(auth_hash(&mk, &salt).unwrap());
        assert_eq!(recomputed, reg.payload.auth_hash);
    }

    #[test]
    fn registration_then_login_unwraps_same_user_key() {
        let reg = prepare_registration_with_params("master-pw", &fast_params()).unwrap();
        let lr = login(
            "master-pw",
            &reg.payload.account_salt,
            &reg.payload.kdf_params,
            &reg.payload.wrapped_user_key,
        )
        .unwrap();
        assert_eq!(*reg.user_key, *lr.user_key);
        assert_eq!(reg.payload.auth_hash, lr.auth_hash);
    }

    #[test]
    fn login_with_wrong_password_fails_to_unwrap() {
        let reg = prepare_registration_with_params("right-pw", &fast_params()).unwrap();
        // LoginResult holds the secret user_key, so it deliberately has no Debug;
        // match on the error instead of .unwrap_err() (which would need Ok: Debug).
        let res = login(
            "wrong-pw",
            &reg.payload.account_salt,
            &reg.payload.kdf_params,
            &reg.payload.wrapped_user_key,
        );
        assert!(matches!(res, Err(CryptoError::Decrypt)));
    }

    #[test]
    fn item_encrypted_after_register_decrypts_after_login() {
        let reg = prepare_registration_with_params("pw", &fast_params()).unwrap();
        let ct = encrypt_item(b"host secret", &reg.user_key, b"host.x").unwrap();
        let lr = login(
            "pw",
            &reg.payload.account_salt,
            &reg.payload.kdf_params,
            &reg.payload.wrapped_user_key,
        )
        .unwrap();
        assert_eq!(
            decrypt_item(&ct, &lr.user_key, b"host.x").unwrap(),
            b"host secret"
        );
    }

    #[test]
    fn recovery_path_unwraps_same_user_key_as_password_path() {
        let reg = prepare_registration_with_params("pw", &fast_params()).unwrap();
        let kit = format_recovery_key(&reg.recovery_key);
        let recovered = recover_user_key(&kit, &reg.payload.recovery_wrapped_user_key).unwrap();
        assert_eq!(*reg.user_key, *recovered);
    }

    #[test]
    fn recovery_with_wrong_kit_fails() {
        let reg = prepare_registration_with_params("pw", &fast_params()).unwrap();
        let wrong_kit = format_recovery_key(&generate_recovery_key());
        let err = recover_user_key(&wrong_kit, &reg.payload.recovery_wrapped_user_key).unwrap_err();
        assert_eq!(err, CryptoError::Decrypt);
    }

    // --- two-device determinism --------------------------------------------

    #[test]
    fn two_devices_same_password_and_salt_match() {
        // Simulate the server handing both devices the same account material.
        let reg = prepare_registration_with_params("shared-pw", &fast_params()).unwrap();
        let salt_b64 = &reg.payload.account_salt;
        let params_json = &reg.payload.kdf_params;
        let wrapped = &reg.payload.wrapped_user_key;

        // Device A (the registering device) already has reg.user_key.
        // Device B logs in fresh with the same password.
        let dev_b = login("shared-pw", salt_b64, params_json, wrapped).unwrap();

        // Same master_key path => same auth_hash and same unwrapped user_key.
        assert_eq!(reg.payload.auth_hash, dev_b.auth_hash);
        assert_eq!(*reg.user_key, *dev_b.user_key);

        // And an item encrypted on A decrypts on B.
        let ct = encrypt_item(b"cross-device item", &reg.user_key, b"host.x").unwrap();
        assert_eq!(
            decrypt_item(&ct, &dev_b.user_key, b"host.x").unwrap(),
            b"cross-device item"
        );
    }

    #[test]
    fn registration_payload_serializes_to_json() {
        let reg = prepare_registration_with_params("pw", &fast_params()).unwrap();
        let json = serde_json::to_string(&reg.payload).unwrap();
        let back: RegistrationPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.account_salt, reg.payload.account_salt);
        assert_eq!(back.auth_hash, reg.payload.auth_hash);
        assert_eq!(back.wrapped_user_key, reg.payload.wrapped_user_key);
    }
}
