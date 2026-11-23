//! Local per-tab route intent, independent of profiles, credentials and proxies.
//!
//! The containing tab uses `#[serde(default)]` for its `route` field. Only a
//! missing field inherits; explicit malformed/unknown JSON becomes Blocked.
//! Connection IDs are opaque lookup keys, never addresses or filesystem paths.
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

pub const MAX_CONNECTION_ID_BYTES: usize = 128;

/// Only engine-owned reason codes cross into user-visible route errors.
/// Never echo arbitrary renderer text, connection addresses or credentials.
pub fn change_error_code(code: Option<&str>) -> &'static str {
    match code {
        Some("capture") => "capture",
        Some("busy") => "busy",
        Some("before_unload") => "before_unload",
        Some("locked") => "locked",
        Some("stale") => "stale",
        Some("timeout") => "timeout",
        Some("profile_unavailable") => "profile_unavailable",
        _ => "unknown",
    }
}
pub fn change_error_message(code: Option<&str>) -> &'static str {
    match change_error_code(code) {
        "capture" => "Остановите доступ к камере, микрофону или экрану и повторите.",
        "busy" => "Завершите загрузку файла или закройте DevTools и повторите.",
        "before_unload" => "Страница не разрешила перезагрузку. Сохраните введённые данные и повторите.",
        "locked" => "Разблокируйте браузер и повторите.",
        "stale" => "Вкладка изменилась. Откройте выбор пути заново.",
        "timeout" => "Подготовка пути заняла слишком много времени. Повторите попытку.",
        "profile_unavailable" => "Не удалось открыть выбранный путь. Попробуйте ещё раз.",
        _ => "Не удалось сменить путь. Попробуйте ещё раз.",
    }
}


#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionId(String);

impl ConnectionId {
    pub fn parse(value: &str) -> Result<Self, RouteBlockReason> {
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > MAX_CONNECTION_ID_BYTES
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        {
            return Err(RouteBlockReason::InvalidConnectionId);
        }
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Even a syntactically valid opaque ID is not useful in routine diagnostics.
impl fmt::Debug for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConnectionId([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteBlockReason {
    UnknownIntent,
    MalformedIntent,
    InvalidConnectionId,
}

impl RouteBlockReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::UnknownIntent => "unknown_intent",
            Self::MalformedIntent => "invalid_shape",
            Self::InvalidConnectionId => "invalid_id",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "unknown_intent" => Some(Self::UnknownIntent),
            "invalid_shape" => Some(Self::MalformedIntent),
            "invalid_id" => Some(Self::InvalidConnectionId),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RouteChoice {
    #[default]
    Inherit,
    Direct,
    Connection {
        id: ConnectionId,
    },
    Blocked {
        reason: RouteBlockReason,
    },
}

impl RouteChoice {
    fn from_wire(value: Value) -> Self {
        let blocked = |reason| Self::Blocked { reason };
        let Some(object) = value.as_object() else {
            return blocked(RouteBlockReason::MalformedIntent);
        };
        let Some(kind) = object.get("kind").and_then(Value::as_str) else {
            return blocked(RouteBlockReason::MalformedIntent);
        };
        match kind {
            "inherit" if object.len() == 1 => Self::Inherit,
            "direct" if object.len() == 1 => Self::Direct,
            "connection" if object.len() == 2 => {
                let Some(id) = object.get("id").and_then(Value::as_str) else {
                    return blocked(RouteBlockReason::MalformedIntent);
                };
                match ConnectionId::parse(id) {
                    Ok(id) => Self::Connection { id },
                    Err(reason) => blocked(reason),
                }
            }
            "blocked" if object.len() == 2 => object
                .get("reason")
                .and_then(Value::as_str)
                .and_then(RouteBlockReason::from_code)
                .map(|reason| blocked(reason))
                .unwrap_or_else(|| blocked(RouteBlockReason::MalformedIntent)),
            "inherit" | "direct" | "connection" | "blocked" => {
                blocked(RouteBlockReason::MalformedIntent)
            }
            _ => blocked(RouteBlockReason::UnknownIntent),
        }
    }

    /// Resolve only membership, without consulting or changing the Super route.
    /// The caller supplies saved connection IDs, not URLs or list positions.
    pub fn resolve<'a, 'b>(
        &'a self,
        existing_ids: impl IntoIterator<Item = &'b str>,
    ) -> Result<ResolvedRoute<'a>, RouteResolveError> {
        match self {
            Self::Inherit => Ok(ResolvedRoute::Inherit),
            Self::Direct => Ok(ResolvedRoute::Direct),
            Self::Blocked { reason } => Err(RouteResolveError::Blocked(*reason)),
            Self::Connection { id } => {
                let mut found = false;
                for existing in existing_ids {
                    if existing == id.as_str() {
                        if found {
                            return Err(RouteResolveError::AmbiguousConnection);
                        }
                        found = true;
                    }
                }
                if found {
                    Ok(ResolvedRoute::Connection { id })
                } else {
                    Err(RouteResolveError::MissingConnection)
                }
            }
        }
    }
}

impl Serialize for RouteChoice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(match self {
            Self::Inherit | Self::Direct => 1,
            _ => 2,
        }))?;
        match self {
            Self::Inherit => map.serialize_entry("kind", "inherit")?,
            Self::Direct => map.serialize_entry("kind", "direct")?,
            Self::Connection { id } => {
                map.serialize_entry("kind", "connection")?;
                map.serialize_entry("id", id.as_str())?;
            }
            Self::Blocked { reason } => {
                map.serialize_entry("kind", "blocked")?;
                map.serialize_entry("reason", reason.code())?;
            }
        }
        map.end()
    }
}

struct RouteVisitor;

impl<'de> Visitor<'de> for RouteVisitor {
    type Value = RouteChoice;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a route intent object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut object = serde_json::Map::new();
        let mut duplicate = false;
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value::<Value>()?;
            duplicate |= object.insert(key, value).is_some();
        }
        if duplicate {
            Ok(RouteChoice::Blocked {
                reason: RouteBlockReason::MalformedIntent,
            })
        } else {
            Ok(RouteChoice::from_wire(Value::Object(object)))
        }
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        self.visit_unit()
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(RouteChoice::Blocked {
            reason: RouteBlockReason::MalformedIntent,
        })
    }

    fn visit_bool<E: serde::de::Error>(self, _value: bool) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_i64<E: serde::de::Error>(self, _value: i64) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_u64<E: serde::de::Error>(self, _value: u64) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_f64<E: serde::de::Error>(self, _value: f64) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_str<E: serde::de::Error>(self, _value: &str) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_string<E: serde::de::Error>(self, _value: String) -> Result<Self::Value, E> {
        self.visit_unit()
    }
}

impl<'de> Deserialize<'de> for RouteChoice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A derived tagged enum would reject the whole containing session.
        // A Value-only decoder would silently accept duplicate kind/id fields.
        deserializer.deserialize_any(RouteVisitor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedRoute<'a> {
    Inherit,
    Direct,
    Connection { id: &'a ConnectionId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteResolveError {
    Blocked(RouteBlockReason),
    MissingConnection,
    AmbiguousConnection,
}

impl fmt::Display for RouteResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Blocked(_) => {
                "Маршрут вкладки недоступен: сохранённый выбор некорректен или не поддерживается."
            }
            Self::MissingConnection => {
                "Маршрут вкладки недоступен: выбранное подключение не найдено."
            }
            Self::AmbiguousConnection => "Маршрут вкладки недоступен: ID подключения не уникален.",
        })
    }
}

impl std::error::Error for RouteResolveError {}

/// Parent-prepared transport only. No credentials or remote addresses cross IPC.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRoute {
    pub key: String,
    pub proxy: String,
    /// Only actual Direct permits WebRTC sockets outside the local HTTP proxy.
    #[serde(default)]
    pub allow_non_proxied_udp: bool,
}
impl fmt::Debug for NativeRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NativeRoute([local transport])")
    }
}
impl NativeRoute {
    /// Strict validation also protects profile path construction and bypass lists.
    pub fn validate(&self) -> bool {
        self.key.len() == 64
            && self.key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && self.proxy.strip_prefix("http://127.0.0.1:")
                .and_then(|p| if p.bytes().all(|b| b.is_ascii_digit()) { p.parse::<u16>().ok() } else { None })
                .is_some_and(|p| p != 0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRouteBinding {
    pub route: RouteChoice,
    pub prepared: Option<NativeRoute>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteBootstrap {
    pub inherit: NativeRoute,
    pub bindings: Vec<NativeRouteBinding>,
}
