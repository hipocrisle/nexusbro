//! Pure single-video admission and bounded public metadata. No fetching, shell,
//! extractor options, cookies, signed stream URLs or playlist expansion here.
//! URL admission is NOT playback permission: the parent must still bind the
//! operation to its actual tab/route/generation and loopback transport.
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

pub const MAX_INPUT_URL_BYTES: usize = 2048;
pub const MAX_TITLE_CHARS: usize = 512;
pub const MAX_AUTHOR_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputError {
    InvalidUrl,
    UnsupportedHost,
    UnsupportedPath,
    InvalidVideoId,
    AmbiguousVideo,
    InvalidMetadata,
}
impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never reflect an untrusted/signed URL, query or extractor diagnostic.
        f.write_str(match self {
            Self::InvalidUrl => "Некорректный адрес видео",
            Self::UnsupportedHost => "Этот адрес не относится к поддерживаемым адресам YouTube",
            Self::UnsupportedPath => "Нужна ссылка на одно видео YouTube",
            Self::InvalidVideoId => "Некорректный идентификатор видео",
            Self::AmbiguousVideo => "Ссылка содержит неоднозначный идентификатор видео",
            Self::InvalidMetadata => "Некорректные публичные данные видео",
        })
    }
}
impl std::error::Error for InputError {}

/// Private representation and validated deserialization prevent unchecked IDs
/// from reaching an extractor argument or the canonical URL builder.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VideoId(String);
impl VideoId {
    pub fn parse(value: &str) -> Result<Self, InputError> {
        if value.len() != 11
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(InputError::InvalidVideoId);
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Drops every tracking, timestamp, playlist and redirect parameter.
    pub fn watch_url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.0)
    }
    /// Recognize only explicitly supported, unambiguous HTTPS video URLs.
    /// A playlist parameter never causes more than this one ID to be returned.
    pub fn from_url(input: &str) -> Result<Self, InputError> {
        if input.len() > MAX_INPUT_URL_BYTES
            || !input.is_ascii()
            || input.bytes().any(|c| c <= b' ' || c == 127 || c == b'\\')
            || !input
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
        {
            return Err(InputError::InvalidUrl);
        }
        let rest = &input[8..];
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        // Url normalizes explicit :443 away. Inspect the raw authority BEFORE
        // parsing so default ports, empty ports and empty userinfo cannot pass.
        if authority.is_empty() || authority.contains(['@', ':', '%']) {
            return Err(InputError::InvalidUrl);
        }
        let url = Url::parse(input).map_err(|_| InputError::InvalidUrl)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err(InputError::InvalidUrl);
        }
        let host = url.host_str().ok_or(InputError::UnsupportedHost)?;
        if !matches!(
            host,
            "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be"
        ) {
            return Err(InputError::UnsupportedHost);
        }
        let suffix = &rest[authority_end..];
        let raw_path = &suffix[..suffix.find(['?', '#']).unwrap_or(suffix.len())];
        // No parser repair (dot segments), encoded slashes/IDs or backslashes.
        if raw_path != url.path() || raw_path.contains('%') {
            return Err(InputError::UnsupportedPath);
        }
        let mut video = None;
        if let Some(query) = url.query() {
            for item in query.split('&') {
                let (key, value) = item.split_once('=').unwrap_or((item, ""));
                // Encoded query keys could disguise a second v. Reject them;
                // ordinary encoded tracking VALUES are irrelevant and dropped.
                if key.contains('%') || key.contains('+') {
                    return Err(InputError::InvalidUrl);
                }
                if key == "v" {
                    if video.is_some() {
                        return Err(InputError::AmbiguousVideo);
                    }
                    video = Some(Self::parse(value)?);
                }
            }
        }
        if host != "youtu.be" && raw_path == "/watch" {
            return video.ok_or(InputError::InvalidVideoId);
        }
        if video.is_some() {
            return Err(InputError::AmbiguousVideo);
        }
        let id = if host == "youtu.be" {
            raw_path.strip_prefix('/').filter(|v| !v.contains('/'))
        } else {
            ["/shorts/", "/live/", "/embed/"]
                .iter()
                .find_map(|prefix| raw_path.strip_prefix(prefix))
                .filter(|v| !v.contains('/'))
        }
        .ok_or(InputError::UnsupportedPath)?;
        Self::parse(id)
    }
}
impl FromStr for VideoId {
    type Err = InputError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
impl TryFrom<String> for VideoId {
    type Error = InputError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}
impl From<VideoId> for String {
    fn from(value: VideoId) -> Self {
        value.0
    }
}

/// Public display data only. No external thumbnail, stream URL or confidential
/// header is accepted in this type. Serialize-only: construct through validation.
#[derive(Clone, Debug, Serialize)]
pub struct PublicMetadata {
    id: VideoId,
    title: String,
    author: Option<String>,
    duration_seconds: Option<u64>,
    is_live: bool,
}
impl PublicMetadata {
    pub fn new(
        id: VideoId,
        title: String,
        author: Option<String>,
        duration_seconds: Option<u64>,
        is_live: bool,
    ) -> Result<Self, InputError> {
        fn valid(value: &str, max: usize) -> bool {
            !value.trim().is_empty()
                && value.chars().count() <= max
                && !value.chars().any(char::is_control)
        }
        if !valid(&title, MAX_TITLE_CHARS)
            || author.as_ref().is_some_and(|v| !valid(v, MAX_AUTHOR_CHARS))
        {
            return Err(InputError::InvalidMetadata);
        }
        Ok(Self {
            id,
            title,
            author,
            duration_seconds,
            is_live,
        })
    }
    pub fn id(&self) -> &VideoId {
        &self.id
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }
    pub fn duration_seconds(&self) -> Option<u64> {
        self.duration_seconds
    }
    pub fn is_live(&self) -> bool {
        self.is_live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "aB3_dE-9xY0";
    #[test]
    fn canonical_id_has_exact_ascii_alphabet_and_length() {
        assert_eq!(VideoId::parse(ID).unwrap().as_str(), ID);
        for value in [
            "",
            "abcdefghij",
            "abcdefghijkl",
            "aB3_dE-9xY!",
            "абвгдеёжзий",
            "aB3_dE-9xY/",
            "aB3_dE-9xY\n",
            "--exec=rm x",
        ] {
            assert!(VideoId::parse(value).is_err(), "unexpected ID acceptance");
        }
    }
    #[test]
    fn all_admitted_hosts_and_paths_reduce_to_one_canonical_watch_url() {
        for host in [
            "youtube.com",
            "www.youtube.com",
            "m.youtube.com",
            "music.youtube.com",
        ] {
            for path in [
                format!("/watch?v={ID}"),
                format!("/shorts/{ID}"),
                format!("/live/{ID}"),
                format!("/embed/{ID}"),
            ] {
                let id = VideoId::from_url(&format!("https://{host}{path}")).unwrap();
                assert_eq!(
                    id.watch_url(),
                    format!("https://www.youtube.com/watch?v={ID}")
                );
            }
        }
        assert_eq!(
            VideoId::from_url(&format!("HTTPS://YOUTU.BE/{ID}?si=tracking&t=10#fragment"))
                .unwrap()
                .as_str(),
            ID
        );
    }
    #[test]
    fn playlist_and_tracking_never_expand_or_survive_normalization() {
        let id=VideoId::from_url(&format!("https://www.youtube.com/watch?v={ID}&list=PLanything&index=99&ab_channel=public%20name")).unwrap();
        assert_eq!(
            id.watch_url(),
            format!("https://www.youtube.com/watch?v={ID}")
        );
        for path in [
            "/playlist?list=PLanything",
            "/watch?list=PLanything",
            "/redirect?q=https://evil.test",
            "/feed/subscriptions",
        ] {
            assert!(VideoId::from_url(&format!("https://youtube.com{path}")).is_err());
        }
    }
    #[test]
    fn authority_scheme_and_parser_repair_attacks_are_rejected() {
        for input in [
            format!("http://youtube.com/watch?v={ID}"),
            format!("https://youtube.com:443/watch?v={ID}"),
            format!("https://youtube.com:/watch?v={ID}"),
            format!("https://youtube.com:8443/watch?v={ID}"),
            format!("https://user@youtube.com/watch?v={ID}"),
            format!("https://@youtube.com/watch?v={ID}"),
            format!("https://youtube.com@evil.test/watch?v={ID}"),
            format!("https://youtube.com.evil.test/watch?v={ID}"),
            format!("https://youtube.com./watch?v={ID}"),
            format!("https://www.youtu.be/{ID}"),
            format!("https://youtube-nocookie.com/embed/{ID}"),
            format!("https://yоutube.com/watch?v={ID}"),
            format!(" https://youtube.com/watch?v={ID}"),
            format!("https://youtube.com\\@evil.test/watch?v={ID}"),
            format!("https://youtube.com/x/../watch?v={ID}"),
            format!("https://youtube.com/%77atch?v={ID}"),
        ] {
            assert!(
                VideoId::from_url(&input).is_err(),
                "unexpected unsafe URL acceptance"
            );
        }
    }
    #[test]
    fn conflicting_or_duplicate_identifiers_and_encoded_keys_are_rejected() {
        for path in [
            format!("/watch?v={ID}&v={ID}"),
            format!("/watch?v={ID}&v=12345678901"),
            format!("/watch?v={ID}&%76=12345678901"),
            format!("/watch?v=%61B3_dE-9xY0"),
            format!("/watch?V={ID}"),
            format!("/shorts/{ID}?v={ID}"),
            format!("/live/{ID}/extra"),
            format!("/embed/{ID}/"),
            format!("/watch?v={ID}%00"),
        ] {
            assert!(VideoId::from_url(&format!("https://youtube.com{path}")).is_err());
        }
        assert!(VideoId::from_url(&format!("https://youtu.be/{ID}?v=12345678901")).is_err());
        assert!(VideoId::from_url(&format!(
            "https://youtube.com/watch?v={ID}&x={}",
            "a".repeat(MAX_INPUT_URL_BYTES)
        ))
        .is_err());
    }
    #[test]
    fn serde_cannot_construct_an_unchecked_video_id() {
        assert!(serde_json::from_str::<VideoId>("\"--exec bad\"").is_err());
        let id: VideoId = serde_json::from_str(&format!("\"{ID}\"")).unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{ID}\""));
    }
    #[test]
    fn metadata_is_bounded_unicode_display_data_and_errors_never_reflect_input() {
        let id = VideoId::parse(ID).unwrap();
        let metadata =
            PublicMetadata::new(id.clone(), "Видео 🎵".into(), None, None, true).unwrap();
        assert!(metadata.is_live());
        assert_eq!(metadata.id(), &id);
        for title in [
            "".to_string(),
            " \t".into(),
            "secret\nheader".into(),
            "я".repeat(MAX_TITLE_CHARS + 1),
        ] {
            assert!(PublicMetadata::new(id.clone(), title, None, None, false).is_err());
        }
        assert!(PublicMetadata::new(
            id,
            "title".into(),
            Some("x".repeat(MAX_AUTHOR_CHARS + 1)),
            Some(123),
            false
        )
        .is_err());
        assert!(!InputError::InvalidUrl.to_string().contains("https"));
    }
}
