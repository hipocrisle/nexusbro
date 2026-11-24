//! Memory-only cookie escrow. This codec is not a native lifecycle acceptance.
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_COOKIES: usize = 4096;
const ERROR: &str = "Cookie escrow refused";
type Result<T> = std::result::Result<T, String>;
fn refusal<T>() -> Result<T> {
    Err(ERROR.into())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub profile_key: String,
    pub context_key: String,
    pub generation: u64,
    pub attempt: u64,
}
impl Scope {
    fn aad(&self) -> Result<Vec<u8>> {
        if [&self.profile_key, &self.context_key].iter().any(|s| {
            s.len() != 64
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return refusal();
        }
        let mut bytes = b"NexusBro cookie escrow v1\0".to_vec();
        bytes.extend(serde_json::to_vec(self).map_err(|_| ERROR)?);
        Ok(bytes)
    }
}
pub struct EphemeralKey(Zeroizing<[u8; 32]>);
impl std::fmt::Debug for EphemeralKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EphemeralKey([redacted])")
    }
}
impl EphemeralKey {
    pub fn generate() -> Result<Self> {
        let mut key = Zeroizing::new([0; 32]);
        OsRng.try_fill_bytes(&mut *key).map_err(|_| ERROR)?;
        Ok(Self(key))
    }
    pub fn from_base64(text: &str) -> Result<Self> {
        if text.len() != 44 {
            return refusal();
        }
        let bytes = Zeroizing::new(STANDARD.decode(text).map_err(|_| ERROR)?);
        let canonical = Zeroizing::new(STANDARD.encode(&*bytes));
        if bytes.len() != 32 || canonical.as_str() != text {
            return refusal();
        }
        let mut key = Zeroizing::new([0; 32]);
        key.copy_from_slice(&bytes);
        Ok(Self(key))
    }
    pub fn wire_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(STANDARD.encode(&*self.0))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sealed {
    pub version: u8,
    pub nonce: String,
    pub ciphertext: String,
}
impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealed([redacted])")
    }
}
pub fn seal(scope: &Scope, key: &EphemeralKey, bytes: &[u8]) -> Result<Sealed> {
    if bytes.len() > MAX_BYTES {
        return refusal();
    }
    let aad = scope.aad()?;
    let mut nonce = [0u8; 24];
    OsRng.try_fill_bytes(&mut nonce).map_err(|_| ERROR)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&*key.0).map_err(|_| ERROR)?;
    let encrypted = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: bytes,
                aad: &aad,
            },
        )
        .map_err(|_| ERROR)?;
    Ok(Sealed {
        version: 1,
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(encrypted),
    })
}
pub fn open(scope: &Scope, key: &EphemeralKey, sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>> {
    if sealed.version != 1
        || sealed.nonce.len() != 32
        || sealed.ciphertext.len() > ((MAX_BYTES + 16 + 2) / 3) * 4
    {
        return refusal();
    }
    let aad = scope.aad()?;
    let nonce = STANDARD.decode(&sealed.nonce).map_err(|_| ERROR)?;
    let cipher = STANDARD.decode(&sealed.ciphertext).map_err(|_| ERROR)?;
    if nonce.len() != 24 || cipher.len() < 16 || cipher.len() > MAX_BYTES + 16 {
        return refusal();
    }
    let engine = XChaCha20Poly1305::new_from_slice(&*key.0).map_err(|_| ERROR)?;
    engine
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &cipher,
                aad: &aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| ERROR.into())
}
// serde_json does not zeroize its strings. Every owned clear tree is guarded.
struct Clear(Value);
fn wipe(value: &mut Value) {
    match value {
        Value::String(s) => s.zeroize(),
        Value::Array(a) => {
            for v in a {
                wipe(v)
            }
        }
        Value::Object(m) => {
            for (_, v) in m.iter_mut() {
                wipe(v)
            }
        }
        _ => {}
    }
}
impl Drop for Clear {
    fn drop(&mut self) {
        wipe(&mut self.0)
    }
}
// Reject duplicate JSON fields instead of silently replacing cookie security data.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bounded cookie JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom(ERROR))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                if v.len() > 65536 {
                    return Err(E::custom(ERROR));
                }
                Ok(Strict(Value::String(v.into())))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                mut v: String,
            ) -> std::result::Result<Strict, E> {
                if v.len() > 65536 {
                    v.zeroize();
                    return Err(E::custom(ERROR));
                }
                Ok(Strict(Value::String(v)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut clear = Clear(Value::Array(Vec::new()));
                while let Some(mut v) = a.next_element::<Strict>()? {
                    if clear.0.as_array().unwrap().len() >= MAX_COOKIES {
                        wipe(&mut v.0);
                        return Err(serde::de::Error::custom(ERROR));
                    }
                    clear.0.as_array_mut().unwrap().push(v.0);
                }
                Ok(Strict(std::mem::replace(&mut clear.0, Value::Null)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut clear = Clear(Value::Object(Map::new()));
                while let Some(mut key) = a.next_key::<String>()? {
                    if key.len() > 128
                        || clear.0.as_object().unwrap().len() >= 32
                        || clear.0.as_object().unwrap().contains_key(&key)
                    {
                        key.zeroize();
                        return Err(serde::de::Error::custom(ERROR));
                    }
                    let v = a.next_value::<Strict>()?;
                    clear.0.as_object_mut().unwrap().insert(key, v.0);
                }
                Ok(Strict(std::mem::replace(&mut clear.0, Value::Null)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
/// Full canonical jar, including persistent cookies, used to detect loss before admission.
/// No Debug, Clone, Serialize or unguarded plaintext accessor.
pub struct CookieSnapshot {
    canonical: Zeroizing<Vec<u8>>,
}
fn text<'a>(m: &'a Map<String, Value>, name: &str, max: usize) -> Result<&'a str> {
    let s = m.get(name).and_then(Value::as_str).ok_or(ERROR)?;
    if s.len() > max || s.chars().any(|c| c.is_control()) {
        return refusal();
    }
    Ok(s)
}
fn boolean(m: &Map<String, Value>, name: &str) -> Result<bool> {
    m.get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| ERROR.into())
}
fn enumeration(m: &Map<String, Value>, name: &str, values: &[&str], optional: bool) -> Result<()> {
    if optional && !m.contains_key(name) {
        return Ok(());
    }
    if !values.contains(&text(m, name, 64)?) {
        return refusal();
    }
    Ok(())
}
fn validate(cookie: &Value) -> Result<()> {
    let m = cookie.as_object().ok_or(ERROR)?;
    const FIELDS: &[&str] = &[
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "priority",
        "sourceScheme",
        "sourcePort",
        "partitionKey",
        "partitionKeyOpaque",
    ];
    if m.keys().any(|k| !FIELDS.contains(&k.as_str())) {
        return refusal();
    }
    let name = text(m, "name", 4096)?;
    text(m, "value", 65536)?;
    let domain = text(m, "domain", 253)?;
    let path = text(m, "path", 4096)?;
    let host = domain.strip_prefix('.').unwrap_or(domain);
    // Deliberately narrower than Chromium's accepted host grammar. Unknown => refuse.
    if name.is_empty()
        || host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
        || !path.starts_with('/')
    {
        return refusal();
    }
    let secure = boolean(m, "secure")?;
    boolean(m, "httpOnly")?;
    let session = boolean(m, "session")?;
    let expires = m
        .get("expires")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or(ERROR)?;
    if (session && expires != -1.0)
        || (!session && expires < 0.0)
        || m.get("size").and_then(Value::as_u64).is_none()
    {
        return refusal();
    }
    // CDP cannot preserve creation time. Recreating unspecified SameSite would
    // renew Chromium's Lax+POST grace period and weaken CSRF protection.
    if session && !m.contains_key("sameSite") {
        return refusal();
    }
    enumeration(m, "sameSite", &["Strict", "Lax", "None"], true)?;
    enumeration(m, "priority", &["Low", "Medium", "High"], false)?;
    enumeration(m, "sourceScheme", &["Unset", "NonSecure", "Secure"], false)?;
    let port = m.get("sourcePort").and_then(Value::as_i64).ok_or(ERROR)?;
    if port != -1 && !(1..=65535).contains(&port) {
        return refusal();
    }
    if let Some(opaque) = m.get("partitionKeyOpaque") {
        if opaque.as_bool() != Some(false) {
            return refusal();
        }
    }
    if let Some(partition) = m.get("partitionKey") {
        if !secure {
            return refusal();
        }
        let p = partition.as_object().ok_or(ERROR)?;
        if p.len() != 2
            || !p.contains_key("topLevelSite")
            || !p.contains_key("hasCrossSiteAncestor")
        {
            return refusal();
        }
        boolean(p, "hasCrossSiteAncestor")?;
        let site = text(p, "topLevelSite", 1024)?;
        let host = site
            .strip_prefix("https://")
            .or_else(|| site.strip_prefix("http://"))
            .ok_or(ERROR)?;
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return refusal();
        }
    }
    if name.starts_with("__Secure-") && !secure {
        return refusal();
    }
    if name.starts_with("__Host-") && (!secure || domain.starts_with('.') || path != "/") {
        return refusal();
    }
    Ok(())
}
fn canonical(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if bytes.len() > MAX_BYTES {
        return refusal();
    }
    let mut clear = Clear(
        serde_json::from_slice::<Strict>(bytes)
            .map_err(|_| ERROR)?
            .0,
    );
    sort_objects(&mut clear.0);
    let root = clear.0.as_object_mut().ok_or(ERROR)?;
    if root.len() != 1 || !root.contains_key("cookies") {
        return refusal();
    }
    let cookies = root
        .get_mut("cookies")
        .and_then(Value::as_array_mut)
        .ok_or(ERROR)?;
    if cookies.len() > MAX_COOKIES {
        return refusal();
    }
    let mut keys = Vec::with_capacity(cookies.len());
    for cookie in cookies.iter() {
        validate(cookie)?;
        keys.push(identity(cookie)?);
    }
    keys.sort();
    if keys.windows(2).any(|p| p[0] == p[1]) {
        return refusal();
    }
    // Identity excludes secret values; deterministic sort independent of CDP iteration order.
    cookies.sort_by_cached_key(|c| identity(c).unwrap_or_default());
    // Omitted and explicit false opaque marker are equivalent, neither loses information.
    for c in cookies.iter_mut() {
        c.as_object_mut().unwrap().remove("partitionKeyOpaque");
    }
    sort_objects(&mut clear.0);
    let serialized = Zeroizing::new(serde_json::to_vec(&clear.0).map_err(|_| ERROR)?);
    if serialized.len() > MAX_BYTES {
        return refusal();
    }
    Ok(serialized)
}
fn sort_objects(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (_, v) in m.iter_mut() {
                sort_objects(v);
            }
            m.sort_keys();
        }
        Value::Array(a) => {
            for v in a {
                sort_objects(v)
            }
        }
        _ => {}
    }
}
fn identity(c: &Value) -> Result<String> {
    serde_json::to_string(&(&c["domain"], &c["path"], &c["name"], &c["partitionKey"]))
        .map_err(|_| ERROR.into())
}
/// Diagnostic metadata only; never an admission or verification result.
#[derive(Debug, Serialize)]
pub struct ReadbackDiagnostic {
    pub expected_count: usize,
    pub actual_count: usize,
    pub missing_count: usize,
    pub extra_count: usize,
    pub parse_refused: bool,
    pub mismatch_fields: Vec<&'static str>,
}
impl CookieSnapshot {
    pub fn from_cdp_result(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            canonical: canonical(bytes)?,
        })
    }
    /// Explicit bounded plaintext for sealing; caller owns the zeroizing buffer.
    pub fn canonical_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.canonical.to_vec())
    }
    /// Wire JSON array of session CookieParam objects, NOT a CDP request envelope.
    pub fn session_params(&self) -> Result<Zeroizing<Vec<u8>>> {
        let clear = Clear(serde_json::from_slice(&self.canonical).map_err(|_| ERROR)?);
        let mut params = Clear(Value::Array(Vec::new()));
        for c in clear.0["cookies"].as_array().ok_or(ERROR)? {
            if c["session"] != true {
                continue;
            }
            let mut p = Map::new();
            for field in [
                "name",
                "value",
                "path",
                "secure",
                "httpOnly",
                "sameSite",
                "priority",
                "sourceScheme",
                "sourcePort",
                "partitionKey",
            ] {
                if let Some(v) = c.get(field) {
                    p.insert(field.into(), v.clone());
                }
            }
            let domain = c["domain"].as_str().ok_or(ERROR)?;
            if domain.starts_with('.') {
                p.insert("domain".into(), Value::String(domain.into()));
            }
            let host = domain.strip_prefix('.').unwrap_or(domain);
            let scheme = if c["sourceScheme"] == "Secure" || c["secure"] == true {
                "https"
            } else {
                "http"
            };
            let port = c["sourcePort"].as_i64().ok_or(ERROR)?;
            let suffix = if port > 0 {
                format!(":{port}")
            } else {
                String::new()
            };
            p.insert(
                "url".into(),
                Value::String(format!("{scheme}://{host}{suffix}/")),
            );
            params.0.as_array_mut().unwrap().push(Value::Object(p));
        }
        let bytes = Zeroizing::new(serde_json::to_vec(&params.0).map_err(|_| ERROR)?);
        if bytes.len() > MAX_BYTES {
            return refusal();
        }
        Ok(bytes)
    }
    /// Expiry-aware full comparison at an explicit caller-provided Unix time.
    /// Only originally persistent cookies with expires <= now may disappear.
    /// Session cookies and every surviving cookie remain exact; extra cookies,
    /// including stale expired cookies in readback, are refused.
    pub fn verify_readback_at(&self, bytes: &[u8], now_unix_seconds: u64) -> Result<()> {
        // All integer seconds must be represented exactly in CDP's f64 clock.
        if now_unix_seconds > 9_007_199_254_740_991 {
            return refusal();
        }
        let actual = canonical(bytes)?;
        let mut expected = Clear(serde_json::from_slice(&self.canonical).map_err(|_| ERROR)?);
        let cookies = expected.0["cookies"].as_array_mut().ok_or(ERROR)?;
        // Wipe removed values before dropping their allocation.
        cookies.retain_mut(|cookie| {
            let expired = cookie["session"] == false
                && cookie["expires"]
                    .as_f64()
                    .is_some_and(|expiry| expiry <= now_unix_seconds as f64);
            if expired {
                wipe(cookie);
            }
            !expired
        });
        let wanted = Zeroizing::new(serde_json::to_vec(&expected.0).map_err(|_| ERROR)?);
        if *wanted != *actual {
            return refusal();
        }
        Ok(())
    }
    /// Reports bounded counts and fixed field labels only. Never accepts data,
    /// changes the snapshot, or exposes cookie identity/value/error payloads.
    pub fn readback_diagnostic_at(
        &self,
        bytes: &[u8],
        now_unix_seconds: u64,
    ) -> ReadbackDiagnostic {
        let mut report = ReadbackDiagnostic {
            expected_count: 0,
            actual_count: 0,
            missing_count: 0,
            extra_count: 0,
            parse_refused: false,
            mismatch_fields: Vec::new(),
        };
        let analyze = (|| -> Result<()> {
            if now_unix_seconds > 9_007_199_254_740_991 {
                return refusal();
            }
            let expected = Clear(serde_json::from_slice(&self.canonical).map_err(|_| ERROR)?);
            let expected = expected.0["cookies"]
                .as_array()
                .ok_or(ERROR)?
                .iter()
                .filter(|c| {
                    !(c["session"] == false
                        && c["expires"]
                            .as_f64()
                            .is_some_and(|v| v <= now_unix_seconds as f64))
                })
                .collect::<Vec<_>>();
            report.expected_count = expected.len();
            let normalized = canonical(bytes)?;
            let actual_tree = Clear(serde_json::from_slice(&normalized).map_err(|_| ERROR)?);
            let actual = actual_tree.0["cookies"].as_array().ok_or(ERROR)?;
            report.actual_count = actual.len();
            // Internal identity comparison borrows zeroizing trees; nothing
            // derived from names/domains/paths is put into the report or logs.
            let same_base = |a: &Value, b: &Value| {
                a["name"] == b["name"] && a["domain"] == b["domain"] && a["path"] == b["path"]
            };
            let same_identity =
                |a: &Value, b: &Value| same_base(a, b) && a["partitionKey"] == b["partitionKey"];
            report.missing_count = expected
                .iter()
                .filter(|e| !actual.iter().any(|a| same_identity(e, a)))
                .count();
            report.extra_count = actual
                .iter()
                .filter(|a| !expected.iter().any(|e| same_identity(e, a)))
                .count();
            const FIELDS: &[&str] = &[
                "secure",
                "httpOnly",
                "sameSite",
                "expires",
                "sourceScheme",
                "sourcePort",
                "priority",
                "session",
                "size",
                "partitionKey",
            ];
            for e in &expected {
                let exact = actual.iter().find(|a| same_identity(e, a));
                // Partition-key drift can change identity. Diagnose it only if
                // the remaining internal base identity is unambiguous.
                let candidate = exact.or_else(|| {
                    let mut matches = actual.iter().filter(|a| same_base(e, a));
                    let first = matches.next()?;
                    if matches.next().is_some()
                        || expected.iter().filter(|other| same_base(e, other)).count() != 1
                    {
                        None
                    } else {
                        Some(first)
                    }
                });
                if let Some(a) = candidate {
                    for &field in FIELDS {
                        if e[field] != a[field] && !report.mismatch_fields.contains(&field) {
                            report.mismatch_fields.push(field);
                        }
                    }
                }
            }
            report.mismatch_fields.sort_unstable();
            Ok(())
        })();
        if analyze.is_err() {
            report.parse_refused = true;
        }
        report
    }
    pub fn verify_readback(&self, bytes: &[u8]) -> Result<()> {
        if *self.canonical != *canonical(bytes)? {
            return refusal();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn scope() -> Scope {
        Scope {
            profile_key: "a".repeat(64),
            context_key: "b".repeat(64),
            generation: 7,
            attempt: 3,
        }
    }
    fn cookie() -> Value {
        json!({"name":"__Host-session","value":"secret-cookie-value","domain":"example.com","path":"/","expires":-1,"size":35,"httpOnly":true,"secure":true,"session":true,"priority":"Medium","sourceScheme":"Secure","sourcePort":443,"sameSite":"None"})
    }
    fn jar(c: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"cookies":[c]})).unwrap()
    }
    #[test]
    fn aead_roundtrip_wire_and_nonce() {
        let key = EphemeralKey::generate().unwrap();
        let wire = key.wire_base64();
        let restored = EphemeralKey::from_base64(&wire).unwrap();
        let one = seal(&scope(), &key, b"session-secret").unwrap();
        let two = seal(&scope(), &key, b"session-secret").unwrap();
        assert_ne!(one.nonce, two.nonce);
        assert_eq!(
            &**open(&scope(), &restored, &one).unwrap(),
            b"session-secret"
        );
        assert_eq!(format!("{key:?}"), "EphemeralKey([redacted])");
        assert!(!format!("{one:?}").contains(&one.ciphertext));
    }
    #[test]
    fn every_scope_component_authenticated() {
        let key = EphemeralKey::generate().unwrap();
        let sealed = seal(&scope(), &key, b"old").unwrap();
        for n in 0..4 {
            let mut s = scope();
            match n {
                0 => s.profile_key = "c".repeat(64),
                1 => s.context_key = "d".repeat(64),
                2 => s.generation += 1,
                _ => s.attempt += 1,
            };
            assert!(open(&s, &key, &sealed).is_err());
        }
        assert!(open(&scope(), &EphemeralKey::generate().unwrap(), &sealed).is_err());
    }
    #[test]
    fn tamper_and_bounds_fail_closed() {
        let key = EphemeralKey::generate().unwrap();
        let mut s = seal(&scope(), &key, b"a").unwrap();
        let mut c = STANDARD.decode(&s.ciphertext).unwrap();
        c[0] ^= 1;
        s.ciphertext = STANDARD.encode(c);
        assert!(open(&scope(), &key, &s).is_err());
        s.version = 2;
        assert!(open(&scope(), &key, &s).is_err());
        s.version = 1;
        s.nonce = "a".repeat(32);
        assert!(open(&scope(), &key, &s).is_err());
        assert!(seal(&scope(), &key, &vec![0; MAX_BYTES + 1]).is_err());
        let payload = vec![42; MAX_BYTES];
        let s = seal(&scope(), &key, &payload).unwrap();
        assert_eq!(&*open(&scope(), &key, &s).unwrap(), &payload);
        let mut bad = scope();
        bad.context_key = "A".repeat(64);
        assert!(seal(&bad, &key, b"").is_err());
        assert!(EphemeralKey::from_base64("not-key").is_err());
    }
    #[test]
    fn host_only_session_params_preserve_security() {
        let raw = jar(cookie());
        let s = CookieSnapshot::from_cdp_result(&raw).unwrap();
        let p: Value = serde_json::from_slice(&s.session_params().unwrap()).unwrap();
        assert_eq!(p[0]["url"], "https://example.com:443/");
        assert!(p[0].get("domain").is_none());
        assert!(p[0].get("expires").is_none());
        for k in [
            "name",
            "value",
            "path",
            "httpOnly",
            "secure",
            "sameSite",
            "priority",
            "sourceScheme",
            "sourcePort",
        ] {
            assert_eq!(p[0][k], cookie()[k]);
        }
        s.verify_readback(&raw).unwrap();
        let mut lost = cookie();
        lost["httpOnly"] = json!(false);
        assert!(s.verify_readback(&jar(lost)).is_err());
    }
    #[test]
    fn domain_partition_and_full_persistent_readback() {
        let mut c = cookie();
        c["name"] = json!("partitioned");
        c["domain"] = json!(".example.com");
        c["path"] = json!("/account");
        c["partitionKey"] =
            json!({"topLevelSite":"https://outer.example","hasCrossSiteAncestor":true});
        let s = CookieSnapshot::from_cdp_result(&jar(c.clone())).unwrap();
        let p: Value = serde_json::from_slice(&s.session_params().unwrap()).unwrap();
        assert_eq!(p[0]["domain"], ".example.com");
        assert_eq!(p[0]["partitionKey"], c["partitionKey"]);
        let mut changed = c.clone();
        changed["partitionKey"]["hasCrossSiteAncestor"] = json!(false);
        assert!(s.verify_readback(&jar(changed)).is_err());
        c["session"] = json!(false);
        c["expires"] = json!(2000000000.25);
        let persistent = CookieSnapshot::from_cdp_result(&jar(c.clone())).unwrap();
        assert_eq!(&*persistent.session_params().unwrap(), b"[]");
        c["expires"] = json!(2000000001.25);
        assert!(persistent.verify_readback(&jar(c)).is_err());
    }
    #[test]
    fn no_silent_schema_or_enum_loss() {
        for (field, value) in [
            ("sameParty", json!(true)),
            ("sourceScheme", json!("future")),
            ("priority", json!("Urgent")),
            ("sameSite", json!("future")),
            ("partitionKeyOpaque", json!(true)),
            ("sourcePort", json!(65536)),
            ("expires", json!(0)),
            ("secure", json!(false)),
        ] {
            let mut c = cookie();
            c[field] = value;
            assert!(CookieSnapshot::from_cdp_result(&jar(c)).is_err(), "{field}");
        }
        let mut c = cookie();
        c["partitionKey"] =
            json!({"topLevelSite":"https://example.com","hasCrossSiteAncestor":false,"future":0});
        assert!(CookieSnapshot::from_cdp_result(&jar(c)).is_err());
        let mut c = cookie();
        c["domain"] = json!(".example.com");
        assert!(CookieSnapshot::from_cdp_result(&jar(c)).is_err());
        let mut c = cookie();
        c.as_object_mut().unwrap().remove("sourceScheme");
        assert!(CookieSnapshot::from_cdp_result(&jar(c)).is_err());
    }
    #[test]
    fn canonical_order_empty_duplicate_and_budget() {
        assert!(CookieSnapshot::from_cdp_result(br#"{"cookies":[],"cookies":[]}"#).is_err());
        let empty = CookieSnapshot::from_cdp_result(br#"{"cookies":[]}"#).unwrap();
        assert_eq!(&*empty.session_params().unwrap(), b"[]");
        assert!(empty.verify_readback(&jar(cookie())).is_err());
        let a = cookie();
        let mut b = cookie();
        b["name"] = json!("second");
        let s = CookieSnapshot::from_cdp_result(
            &serde_json::to_vec(&json!({"cookies":[a.clone(),b.clone()]})).unwrap(),
        )
        .unwrap();
        s.verify_readback(&serde_json::to_vec(&json!({"cookies":[b,a.clone()]})).unwrap())
            .unwrap();
        assert!(CookieSnapshot::from_cdp_result(
            &serde_json::to_vec(&json!({"cookies":[a.clone(),a]})).unwrap()
        )
        .is_err());
        assert!(CookieSnapshot::from_cdp_result(&vec![b' '; MAX_BYTES + 1]).is_err());
        assert!(CookieSnapshot::from_cdp_result(br#"{"cookies":[],"unknown":0}"#).is_err());
        let mut c = cookie();
        c["partitionKeyOpaque"] = json!(false);
        CookieSnapshot::from_cdp_result(&jar(c))
            .unwrap()
            .verify_readback(&jar(cookie()))
            .unwrap();
    }
    #[test]
    fn parser_refuses_duplicate_cookie_fields_and_nested_unknowns() {
        let raw = String::from_utf8(jar(cookie())).unwrap();
        let duplicate = raw.replace("\"secure\":true", "\"secure\":false,\"secure\":true");
        assert_ne!(raw, duplicate);
        assert!(CookieSnapshot::from_cdp_result(duplicate.as_bytes()).is_err());
        let mut c = cookie();
        c["partitionKey"] = serde_json::json!({"topLevelSite":"https://example.com/path","hasCrossSiteAncestor":false});
        assert!(CookieSnapshot::from_cdp_result(&jar(c)).is_err());
        let mut c = cookie();
        c["value"] = Value::String("x".repeat(65537));
        assert!(CookieSnapshot::from_cdp_result(&jar(c)).is_err());
    }
    #[test]
    fn unspecified_samesite_session_refused_without_changing_persistent_policy() {
        let mut c = cookie();
        c.as_object_mut().unwrap().remove("sameSite");
        assert!(CookieSnapshot::from_cdp_result(&jar(c.clone())).is_err());
        c["session"] = json!(false);
        c["expires"] = json!(1000.5);
        let snapshot = CookieSnapshot::from_cdp_result(&jar(c)).unwrap();
        assert_eq!(&*snapshot.session_params().unwrap(), b"[]");
        for policy in ["Strict", "Lax", "None"] {
            let mut c = cookie();
            c["sameSite"] = json!(policy);
            let snapshot = CookieSnapshot::from_cdp_result(&jar(c)).unwrap();
            let params: Value =
                serde_json::from_slice(&snapshot.session_params().unwrap()).unwrap();
            assert_eq!(params[0]["sameSite"], policy);
        }
    }
    #[test]
    fn expiry_aware_readback_only_allows_proven_natural_expiration() {
        let session = cookie();
        let mut persistent = cookie();
        persistent["name"] = json!("persistent");
        persistent["session"] = json!(false);
        persistent["expires"] = json!(1000.5);
        let original =
            serde_json::to_vec(&json!({"cookies":[session.clone(),persistent.clone()]})).unwrap();
        let snapshot = CookieSnapshot::from_cdp_result(&original).unwrap();
        snapshot.verify_readback_at(&original, 1000).unwrap();
        assert!(snapshot
            .verify_readback_at(&jar(session.clone()), 1000)
            .is_err());
        snapshot
            .verify_readback_at(&jar(session.clone()), 1001)
            .unwrap();
        assert!(snapshot
            .verify_readback_at(br#"{"cookies":[]}"#, 1001)
            .is_err());
        assert!(snapshot.verify_readback_at(&original, 1001).is_err());
        let mut changed = session.clone();
        changed["sourcePort"] = json!(8443);
        assert!(snapshot.verify_readback_at(&jar(changed), 1001).is_err());
        assert!(snapshot.verify_readback_at(&jar(persistent), 1001).is_err());
        assert!(snapshot
            .verify_readback_at(&jar(session), u64::MAX)
            .is_err());
    }
    #[test]
    fn expiration_boundary_is_inclusive_and_never_applies_to_session_cookies() {
        let mut c = cookie();
        c["session"] = json!(false);
        c["expires"] = json!(1234);
        let snapshot = CookieSnapshot::from_cdp_result(&jar(c)).unwrap();
        assert!(snapshot
            .verify_readback_at(br#"{"cookies":[]}"#, 1233)
            .is_err());
        snapshot
            .verify_readback_at(br#"{"cookies":[]}"#, 1234)
            .unwrap();
        let session = CookieSnapshot::from_cdp_result(&jar(cookie())).unwrap();
        assert!(session
            .verify_readback_at(br#"{"cookies":[]}"#, 9000000000)
            .is_err());
    }
    #[test]
    fn readback_diagnostic_is_redacted_and_does_not_authorize_field_loss() {
        let original = jar(cookie());
        let snapshot = CookieSnapshot::from_cdp_result(&original).unwrap();
        let mut changed = cookie();
        changed["sourcePort"] = json!(8443);
        changed["httpOnly"] = json!(false);
        let changed = jar(changed);
        let diagnostic = snapshot.readback_diagnostic_at(&changed, 100);
        assert!(!diagnostic.parse_refused);
        assert_eq!(diagnostic.expected_count, 1);
        assert_eq!(diagnostic.actual_count, 1);
        assert_eq!(diagnostic.missing_count, 0);
        assert_eq!(diagnostic.extra_count, 0);
        assert_eq!(diagnostic.mismatch_fields, vec!["httpOnly", "sourcePort"]);
        let serialized = serde_json::to_string(&diagnostic).unwrap();
        for forbidden in [
            "secret-cookie-value",
            "__Host-session",
            "example.com",
            "domain",
            "name",
            "value",
            "path",
        ] {
            assert!(!serialized.contains(forbidden));
        }
        assert!(snapshot.verify_readback_at(&changed, 100).is_err());
        snapshot.verify_readback_at(&original, 100).unwrap();
        let mut unknown = cookie();
        unknown["malicious-secret-field"] = json!("LEAK-ME");
        let report = snapshot.readback_diagnostic_at(&jar(unknown), 100);
        assert!(report.parse_refused);
        assert!(report.mismatch_fields.is_empty());
        let wire = serde_json::to_string(&report).unwrap();
        assert!(!wire.contains("malicious"));
        assert!(!wire.contains("LEAK"));
    }
    #[test]
    fn readback_diagnostic_distinguishes_expiry_and_partition_identity_drift() {
        let mut persistent = cookie();
        persistent["session"] = json!(false);
        persistent["expires"] = json!(1000);
        persistent["name"] = json!("persistent");
        let original = serde_json::to_vec(&json!({"cookies":[cookie(),persistent]})).unwrap();
        let snapshot = CookieSnapshot::from_cdp_result(&original).unwrap();
        let before = snapshot.readback_diagnostic_at(&jar(cookie()), 999);
        assert_eq!(before.expected_count, 2);
        assert_eq!(before.missing_count, 1);
        let after = snapshot.readback_diagnostic_at(&jar(cookie()), 1000);
        assert_eq!(after.expected_count, 1);
        assert_eq!(after.missing_count, 0);
        assert_eq!(after.extra_count, 0);
        let mut partitioned = cookie();
        partitioned["partitionKey"] =
            json!({"topLevelSite":"https://outer.example","hasCrossSiteAncestor":true});
        let snapshot = CookieSnapshot::from_cdp_result(&jar(partitioned.clone())).unwrap();
        partitioned["partitionKey"]["hasCrossSiteAncestor"] = json!(false);
        let raw = jar(partitioned);
        let report = snapshot.readback_diagnostic_at(&raw, 1000);
        assert_eq!(report.missing_count, 1);
        assert_eq!(report.extra_count, 1);
        assert_eq!(report.mismatch_fields, vec!["partitionKey"]);
        assert!(snapshot.verify_readback_at(&raw, 1000).is_err());
        assert!(
            snapshot
                .readback_diagnostic_at(&vec![b' '; MAX_BYTES + 1], 1000)
                .parse_refused
        );
    }
}
