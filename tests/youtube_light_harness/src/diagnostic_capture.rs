//! Optional harness-only hybrid encryption. No operational private key or
//! plaintext file is accepted/created here. Not compiled into the desktop app.
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce,
};
use nexusbro_youtube_light_verification::youtube_light_sidecar::{ExtractFailure, STDERR_LIMIT};
use rand_core::{OsRng, RngCore};
use rsa::{pkcs8::DecodePublicKey, traits::PublicKeyParts, Oaep, RsaPublicKey};
use sha2::Sha256;
use std::{io::Write, path::Path};
use zeroize::Zeroizing;
const AAD: &[u8] = b"nexusbro-extractor-debug-v1";
const REFUSED: &str = "Encrypted diagnostic refused";
#[derive(serde::Serialize)]
struct Envelope {
    schema: u8,
    algorithm: &'static str,
    aad: &'static str,
    wrapped_key: String,
    nonce: String,
    ciphertext: String,
}
fn seal(bytes: &[u8], encoded_public_key: &str) -> Result<Envelope, &'static str> {
    if bytes.is_empty()
        || bytes.len() > STDERR_LIMIT
        || encoded_public_key.is_empty()
        || encoded_public_key.len() > 8192
    {
        return Err(REFUSED);
    }
    let der = STANDARD.decode(encoded_public_key).map_err(|_| REFUSED)?;
    let public_key = RsaPublicKey::from_public_key_der(&der).map_err(|_| REFUSED)?;
    if !(2048..=4096).contains(&public_key.n().bits()) {
        return Err(REFUSED);
    }
    let mut key = Zeroizing::new([0u8; 32]);
    let mut nonce = [0u8; 12];
    OsRng.try_fill_bytes(&mut *key).map_err(|_| REFUSED)?;
    OsRng.try_fill_bytes(&mut nonce).map_err(|_| REFUSED)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&*key).map_err(|_| REFUSED)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: bytes,
                aad: AAD,
            },
        )
        .map_err(|_| REFUSED)?;
    let wrapped = public_key
        .encrypt(&mut OsRng, Oaep::new::<Sha256>(), &*key)
        .map_err(|_| REFUSED)?;
    Ok(Envelope {
        schema: 1,
        algorithm: "RSA-OAEP-SHA256+ChaCha20Poly1305",
        aad: "nexusbro-extractor-debug-v1",
        wrapped_key: STANDARD.encode(wrapped),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
}
fn write_ciphertext(path: &Path, envelope: &Envelope) -> Result<(), &'static str> {
    if !path.is_absolute() || path.as_os_str().len() > 4096 || path.exists() {
        return Err(REFUSED);
    }
    let parent = path.parent().ok_or(REFUSED)?;
    if !std::fs::symlink_metadata(parent)
        .map_err(|_| REFUSED)?
        .file_type()
        .is_dir()
    {
        return Err(REFUSED);
    }
    let bytes = serde_json::to_vec(envelope).map_err(|_| REFUSED)?;
    if bytes.len() > 128 * 1024 {
        return Err(REFUSED);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Only already encrypted material can reach this file descriptor.
    let mut file = options.open(path).map_err(|_| REFUSED)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| REFUSED)
}
pub fn write_failure(failure: &ExtractFailure) -> Result<bool, &'static str> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Ok(false);
    }
    let encoded = match std::env::var("NEXUSBRO_TEST_DIAGNOSTIC_PUBLIC_KEY") {
        Ok(v) if !v.is_empty() && v.len() <= 8192 => v,
        Ok(v) if v.is_empty() => return Ok(false),
        Err(std::env::VarError::NotPresent) => return Ok(false),
        _ => return Err(REFUSED),
    };
    let Some(bytes) = failure.ci_stderr() else {
        return Ok(false);
    };
    let output = std::env::var_os("NEXUSBRO_TEST_DIAGNOSTIC_OUTPUT").ok_or(REFUSED)?;
    if output.len() > 4096 {
        return Err(REFUSED);
    }
    let envelope = seal(bytes, &encoded)?;
    write_ciphertext(Path::new(&output), &envelope)?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    use rsa::{pkcs8::EncodePublicKey, RsaPrivateKey};
    #[test]
    fn hybrid_roundtrip_and_tamper_are_authenticated_without_plaintext_artifacts() {
        // Disposable mathematical test key only; NEVER the operational
        // diagnostic recipient key. Generated in RAM and not printed/written.
        let private = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let public = RsaPublicKey::from(&private).to_public_key_der().unwrap();
        let encoded = STANDARD.encode(public.as_bytes());
        let secret =
            Zeroizing::new(b"ERROR: confidential https://private.invalid/?token=SECRET".to_vec());
        let envelope = seal(&secret, &encoded).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();
        assert!(!wire.contains("confidential"));
        assert!(!wire.contains("SECRET"));
        assert!(!wire.contains("private.invalid"));
        let wrapped = STANDARD.decode(&envelope.wrapped_key).unwrap();
        let key = Zeroizing::new(private.decrypt(Oaep::new::<Sha256>(), &wrapped).unwrap());
        assert_eq!(key.len(), 32);
        let cipher = ChaCha20Poly1305::new_from_slice(&key).unwrap();
        let nonce = STANDARD.decode(&envelope.nonce).unwrap();
        let mut data = STANDARD.decode(&envelope.ciphertext).unwrap();
        let clear = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &data,
                        aad: AAD,
                    },
                )
                .unwrap(),
        );
        assert_eq!(&*clear, &*secret);
        assert!(cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &data,
                    aad: b"wrong-context"
                }
            )
            .is_err());
        data[0] ^= 1;
        assert!(cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &data,
                    aad: AAD
                }
            )
            .is_err());
        assert!(seal(&vec![0; STDERR_LIMIT + 1], &encoded).is_err());
    }
    #[test]
    fn malformed_keys_return_only_static_redacted_failure() {
        for key in ["", "not-base64:PRIVATE", "U0VDUkVU", &"A".repeat(8193)] {
            assert!(matches!(seal(b"secret", key), Err(REFUSED)));
        }
    }
}
