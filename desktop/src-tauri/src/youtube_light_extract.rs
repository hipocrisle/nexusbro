//! Bounded public yt-dlp JSON admission; no extractor invocation or networking.
//! Caller MUST cap stdout at MAX_JSON_BYTES while reading (not after capture),
//! zeroize its raw capture, and bind expected ID + operation to the actual route.
//! Signed URLs/headers stay RAM-only: no Serialize/Display or unredacted Debug.
//! Only direct HTTPS media is admitted. No manifest/DRM/playlist support is
//! inferred, and codec metadata does NOT assert decoder/MSE compatibility.
use crate::youtube_light::{PublicMetadata, VideoId};
use reqwest::Url;
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::{collections::BTreeMap, fmt, sync::Arc};
use zeroize::Zeroizing;

pub const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FORMATS: usize = 256;
const MAX_URL_BYTES: usize = 16384;
const MAX_HEADERS: usize = 16;
const MAX_HEADER_VALUE: usize = 2048;
const MAX_DURATION: f64 = 7.0 * 24.0 * 60.0 * 60.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractError {
    Oversized,
    InvalidJson,
    WrongVideo,
    UnsupportedVideo,
    InvalidMetadata,
    InvalidFormat,
    UnsafeUrl,
    UnsafeHeaders,
    NoSupportedFormats,
}
impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Не удалось безопасно прочитать публичные данные видео")
    }
}
impl std::error::Error for ExtractError {}

pub struct SignedUrl(Zeroizing<String>);
impl SignedUrl {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
    fn new(value: SecretText) -> Result<Self, ExtractError> {
        let input = value.0;
        if input.is_empty()
            || input.len() > MAX_URL_BYTES
            || !input.is_ascii()
            || input.bytes().any(|b| b <= b' ' || b == 127 || b == b'\\')
            || !input.starts_with("https://")
        {
            return Err(ExtractError::UnsafeUrl);
        }
        let rest = &input[8..];
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..end];
        if authority.contains(['@', ':', '%']) {
            return Err(ExtractError::UnsafeUrl);
        }
        let url = Url::parse(&input).map_err(|_| ExtractError::UnsafeUrl)?;
        let host = url.host_str().ok_or(ExtractError::UnsafeUrl)?;
        if !host.ends_with(".googlevideo.com")
            || host.len() > 253
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || !label.as_bytes()[0].is_ascii_alphanumeric()
                    || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                    || !label
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
            || url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.fragment().is_some()
            || url.path() != "/videoplayback"
            || url.query().is_none_or(str::is_empty)
        {
            return Err(ExtractError::UnsafeUrl);
        }
        // Reject dot/path repair rather than accidentally admitting a different
        // endpoint than the exact direct GoogleVideo playback path.
        let path = &rest[end..];
        let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
        if path != "/videoplayback" {
            return Err(ExtractError::UnsafeUrl);
        }
        Ok(Self(input))
    }
}
impl fmt::Debug for SignedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SignedUrl([redacted])")
    }
}
pub struct AllowedHeaders(BTreeMap<String, Zeroizing<String>>);
impl AllowedHeaders {
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}
impl fmt::Debug for AllowedHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AllowedHeaders([redacted])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Muxed,
    VideoOnly,
    AudioOnly,
}
pub struct MediaFormat {
    pub format_id: String,
    pub ext: String,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub note: Option<String>,
    pub kind: MediaKind,
    pub url: SignedUrl,
    pub headers: Arc<AllowedHeaders>,
}
impl fmt::Debug for MediaFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaFormat")
            .field("format_id", &self.format_id)
            .field("kind", &self.kind)
            .field("url", &"[redacted]")
            .field("headers", &"[redacted]")
            .finish()
    }
}
pub struct ExtractedVideo {
    pub metadata: PublicMetadata,
    pub formats: Vec<MediaFormat>,
    /// Entries deliberately omitted: manifests, DRM, storyboards/other protocol
    /// or unsupported containers. Never presented as playable quality choices.
    pub rejected_formats: usize,
    pub captions: Vec<Arc<crate::youtube_light_captions::CaptionTrack>>,
    pub rejected_captions: usize,
}
impl fmt::Debug for ExtractedVideo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractedVideo")
            .field("formats", &self.formats.len())
            .field("rejected_formats", &self.rejected_formats)
            .finish()
    }
}

struct SecretText(Zeroizing<String>);
impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Zeroizing::new(String::deserialize(d)?);
        if value.len() > MAX_URL_BYTES {
            return Err(de::Error::custom("bounded secret field"));
        }
        Ok(Self(value))
    }
}
#[derive(Default)]
struct RawHeaders(BTreeMap<String, SecretText>);
impl<'de> Deserialize<'de> for RawHeaders {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Headers;
        impl<'de> Visitor<'de> for Headers {
            type Value = RawHeaders;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bounded header map")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut result = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if result.len() >= MAX_HEADERS || key.len() > 64 || !key.is_ascii() {
                        return Err(de::Error::custom("bounded headers"));
                    }
                    let key = key.to_ascii_lowercase();
                    if result.contains_key(&key) {
                        return Err(de::Error::custom("duplicate header"));
                    }
                    result.insert(key, map.next_value::<SecretText>()?);
                }
                Ok(RawHeaders(result))
            }
        }
        d.deserialize_map(Headers)
    }
}
#[derive(Deserialize)]
struct RawFormat {
    format_id: String,
    ext: String,
    vcodec: Option<String>,
    acodec: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    format_note: Option<String>,
    protocol: Option<String>,
    url: Option<SecretText>,
    #[serde(default)]
    http_headers: RawHeaders,
    has_drm: Option<bool>,
    drm_family: Option<de::IgnoredAny>,
    manifest_url: Option<de::IgnoredAny>,
    fragments: Option<de::IgnoredAny>,
    fragment_base_url: Option<de::IgnoredAny>,
}
struct RawFormats(Vec<RawFormat>);
impl<'de> Deserialize<'de> for RawFormats {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Formats;
        impl<'de> Visitor<'de> for Formats {
            type Value = RawFormats;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bounded formats array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut result = Vec::new();
                while result.len() < MAX_FORMATS {
                    match seq.next_element::<RawFormat>()? {
                        Some(value) => result.push(value),
                        None => return Ok(RawFormats(result)),
                    }
                }
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("format count bound"));
                }
                Ok(RawFormats(result))
            }
        }
        d.deserialize_seq(Formats)
    }
}
#[derive(Deserialize)]
struct RawVideo {
    id: String,
    title: String,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    is_live: bool,
    #[serde(rename = "_type")]
    kind: Option<String>,
    entries: Option<de::IgnoredAny>,
    has_drm: Option<bool>,
    #[serde(default)]
    http_headers: RawHeaders,
    formats: RawFormats,
    #[serde(default)]
    subtitles: crate::youtube_light_captions::Sources,
    #[serde(default)]
    automatic_captions: crate::youtube_light_captions::Sources,
}
fn headers(raw: RawHeaders, expected: &VideoId) -> Result<AllowedHeaders, ExtractError> {
    let mut values = BTreeMap::new();
    for (name, value) in raw.0 {
        if !matches!(
            name.as_str(),
            "user-agent" | "accept" | "accept-language" | "origin" | "referer" | "sec-fetch-mode"
        ) || value.0.len() > MAX_HEADER_VALUE
            || value.0.bytes().any(|b| !(32..=126).contains(&b))
        {
            return Err(ExtractError::UnsafeHeaders);
        }
        if name == "origin" && value.0.as_str() != "https://www.youtube.com" {
            return Err(ExtractError::UnsafeHeaders);
        }
        if name == "referer"
            && value.0.as_str() != "https://www.youtube.com/"
            && value.0.as_str() != expected.watch_url()
        {
            return Err(ExtractError::UnsafeHeaders);
        }
        if name == "sec-fetch-mode" && !matches!(value.0.as_str(), "navigate" | "cors" | "no-cors")
        {
            return Err(ExtractError::UnsafeHeaders);
        }
        values.insert(name, value.0);
    }
    Ok(AllowedHeaders(values))
}
fn codec(value: Option<String>) -> Result<Option<String>, ExtractError> {
    let Some(value) = value else { return Ok(None) };
    if value == "none" {
        return Ok(None);
    }
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        return Err(ExtractError::InvalidFormat);
    }
    Ok(Some(value))
}
/// Parse one bounded video result. Unknown yt-dlp fields are ignored, but every
/// field used for playback is typed/checked. No arbitrary top-level URL fallback.
pub fn parse(bytes: &[u8], expected: &VideoId) -> Result<ExtractedVideo, ExtractError> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(ExtractError::Oversized);
    }
    let raw: RawVideo = serde_json::from_slice(bytes).map_err(|_| ExtractError::InvalidJson)?;
    if VideoId::parse(&raw.id).map_err(|_| ExtractError::WrongVideo)? != *expected {
        return Err(ExtractError::WrongVideo);
    }
    if raw.entries.is_some()
        || raw.kind.as_deref().is_some_and(|kind| kind != "video")
        || raw.has_drm == Some(true)
    {
        return Err(ExtractError::UnsupportedVideo);
    }
    let duration = match raw.duration {
        Some(v) if v.is_finite() && v > 0.0 && v <= MAX_DURATION => Some(v.ceil() as u64),
        None => None,
        _ => return Err(ExtractError::InvalidMetadata),
    };
    let metadata = PublicMetadata::new(
        expected.clone(),
        raw.title,
        raw.channel.or(raw.uploader),
        duration,
        raw.is_live,
    )
    .map_err(|_| ExtractError::InvalidMetadata)?;
    let base = Arc::new(headers(raw.http_headers, expected)?);
    let mut formats = Vec::new();
    let mut rejected = 0;
    let mut ids = std::collections::BTreeSet::new();
    for raw in raw.formats.0 {
        if raw.has_drm == Some(true)
            || raw.drm_family.is_some()
            || raw.manifest_url.is_some()
            || raw.fragments.is_some()
            || raw.fragment_base_url.is_some()
            || raw.protocol.as_deref() != Some("https")
            || !matches!(raw.ext.as_str(), "webm" | "mp4" | "m4a")
        {
            rejected += 1;
            continue;
        }
        if raw.format_id.is_empty()
            || raw.format_id.len() > 64
            || raw.format_id.starts_with('-')
            || !raw
                .format_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            || !ids.insert(raw.format_id.clone())
        {
            return Err(ExtractError::InvalidFormat);
        }
        let vcodec = codec(raw.vcodec)?;
        let acodec = codec(raw.acodec)?;
        let kind = match (vcodec.is_some(), acodec.is_some()) {
            (true, true) => MediaKind::Muxed,
            (true, false) => MediaKind::VideoOnly,
            (false, true) => MediaKind::AudioOnly,
            // yt-dlp can include a typed HTTPS entry which carries no media
            // codecs. It is not a playback candidate; never use its URL as a
            // fallback or let it invalidate independently valid media tracks.
            (false, false) => {
                rejected += 1;
                continue;
            }
        };
        if raw.width.is_some_and(|n| n == 0 || n > 16384)
            || raw.height.is_some_and(|n| n == 0 || n > 16384)
            || raw
                .fps
                .is_some_and(|n| !n.is_finite() || n <= 0.0 || n > 240.0)
            || raw
                .format_note
                .as_ref()
                .is_some_and(|n| n.len() > 256 || n.chars().any(char::is_control))
        {
            return Err(ExtractError::InvalidFormat);
        }
        let url = SignedUrl::new(raw.url.ok_or(ExtractError::InvalidFormat)?)?;
        let format_headers = if raw.http_headers.0.is_empty() {
            base.clone()
        } else {
            let specific = headers(raw.http_headers, expected)?;
            let mut combined = base.0.clone();
            combined.extend(specific.0);
            Arc::new(AllowedHeaders(combined))
        };
        formats.push(MediaFormat {
            format_id: raw.format_id,
            ext: raw.ext,
            vcodec,
            acodec,
            width: raw.width,
            height: raw.height,
            fps: raw.fps,
            note: raw.format_note,
            kind,
            url,
            headers: format_headers,
        });
    }
    if formats.is_empty() {
        return Err(ExtractError::NoSupportedFormats);
    }
    let (captions, rejected_captions) =
        crate::youtube_light_captions::tracks(expected, raw.subtitles, raw.automatic_captions);
    Ok(ExtractedVideo {
        metadata,
        formats,
        rejected_formats: rejected,
        captions,
        rejected_captions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn id() -> VideoId {
        VideoId::parse("aB3_dE-9xY0").unwrap()
    }
    fn format() -> Value {
        json!({"format_id":"248","ext":"webm","vcodec":"vp9","acodec":"none","width":1920,"height":1080,"fps":30.0,"format_note":"1080p","protocol":"https","url":"https://rr1---sn-test.googlevideo.com/videoplayback?sig=RAM_ONLY_SECRET","http_headers":{"User-Agent":"fixture","Origin":"https://www.youtube.com"}})
    }
    fn video() -> Value {
        json!({"id":id().as_str(),"title":"Public video","channel":"Public channel","duration":123.25,"is_live":false,"formats":[format()]})
    }
    fn run(value: Value) -> Result<ExtractedVideo, ExtractError> {
        parse(&serde_json::to_vec(&value).unwrap(), &id())
    }
    #[test]
    fn direct_formats_preserve_muxed_and_separate_codec_information() {
        let mut value = video();
        let mut audio = format();
        audio["format_id"] = json!("251");
        audio["vcodec"] = json!("none");
        audio["acodec"] = json!("opus");
        audio["width"] = Value::Null;
        audio["height"] = Value::Null;
        audio["fps"] = Value::Null;
        let mut muxed = format();
        muxed["format_id"] = json!("18");
        muxed["ext"] = json!("mp4");
        muxed["vcodec"] = json!("avc1.42001E");
        muxed["acodec"] = json!("mp4a.40.2");
        value["formats"] = json!([format(), audio, muxed]);
        let result = run(value).unwrap();
        assert_eq!(
            result.formats.iter().map(|f| f.kind).collect::<Vec<_>>(),
            [MediaKind::VideoOnly, MediaKind::AudioOnly, MediaKind::Muxed]
        );
        assert_eq!(result.metadata.duration_seconds(), Some(124));
        assert_eq!(result.formats[1].acodec.as_deref(), Some("opus"));
    }
    #[test]
    fn no_codec_entries_do_not_invalidate_independent_vp9_and_opus_tracks() {
        for absent in [Value::Null, json!("none")] {
            let mut nonmedia = format();
            nonmedia["format_id"] = json!("nonmedia");
            nonmedia["vcodec"] = absent.clone();
            nonmedia["acodec"] = absent;
            let mut audio = format();
            audio["format_id"] = json!("251");
            audio["vcodec"] = json!("none");
            audio["acodec"] = json!("opus");
            audio["width"] = Value::Null;
            audio["height"] = Value::Null;
            audio["fps"] = Value::Null;
            for candidates in [
                json!([nonmedia.clone(), format(), audio.clone()]),
                json!([format(), audio.clone(), nonmedia.clone()]),
            ] {
                let mut value = video();
                value["formats"] = candidates;
                let result = run(value).unwrap();
                assert_eq!(result.rejected_formats, 1);
                assert_eq!(result.formats.len(), 2);
                assert_eq!(result.formats[0].vcodec.as_deref(), Some("vp9"));
                assert_eq!(result.formats[1].acodec.as_deref(), Some("opus"));
            }

            let mut value = video();
            value["formats"] = json!([nonmedia.clone()]);
            assert_eq!(run(value).unwrap_err(), ExtractError::NoSupportedFormats);

            // Rejecting a nonmedia entry does not make an unsafe real media
            // URL, malformed codec, or malformed media dimension acceptable.
            for (field, invalid, expected) in [
                (
                    "url",
                    json!("https://127.0.0.1/videoplayback?x=1"),
                    ExtractError::UnsafeUrl,
                ),
                ("vcodec", json!("bad\ncodec"), ExtractError::InvalidFormat),
                ("width", json!(0), ExtractError::InvalidFormat),
            ] {
                let mut media = format();
                media[field] = invalid;
                let mut value = video();
                value["formats"] = json!([nonmedia.clone(), media]);
                assert_eq!(run(value).unwrap_err(), expected);
            }
        }
    }

    #[test]
    fn forbidden_network_targets_and_url_normalization_attacks_fail() {
        for url in [
            "http://rr1.googlevideo.com/videoplayback?x=1",
            "https://googlevideo.com.evil.test/videoplayback?x=1",
            "https://evilgooglevideo.com/videoplayback?x=1",
            "https://127.0.0.1/videoplayback?x=1",
            "https://localhost/videoplayback?x=1",
            "https://user@rr1.googlevideo.com/videoplayback?x=1",
            "https://rr1.googlevideo.com:443/videoplayback?x=1",
            "https://rr1.googlevideo.com./videoplayback?x=1",
            "https://rr1..googlevideo.com/videoplayback?x=1",
            "https://rr1.googlevideo.com/x/../videoplayback?x=1",
            "https://rr1.googlevideo.com/redirect?url=https://localhost/",
            "https://rr1.googlevideo.com/videoplayback?x=1#fragment",
        ] {
            let mut value = video();
            value["formats"][0]["url"] = json!(url);
            assert_eq!(run(value).unwrap_err(), ExtractError::UnsafeUrl);
        }
    }
    #[test]
    fn headers_cannot_export_credentials_smuggle_or_select_local_origin() {
        for (name, value) in [
            ("Cookie", "secret"),
            ("Authorization", "Bearer secret"),
            ("Host", "localhost"),
            ("Proxy-Authorization", "secret"),
            ("Range", "bytes=0-"),
            ("X-Forwarded-For", "127.0.0.1"),
            ("Origin", "http://localhost"),
            ("Referer", "https://evil.test"),
            ("User-Agent", "good\r\nCookie: secret"),
        ] {
            let mut data = video();
            data["formats"][0]["http_headers"] = json!({(name):value});
            assert_eq!(run(data).unwrap_err(), ExtractError::UnsafeHeaders);
        }
        let mut data = video();
        data["http_headers"] = json!({"Accept":"*/*","Accept-Language":"en"});
        let parsed = run(data).unwrap();
        assert!(parsed.formats[0]
            .headers
            .iter()
            .any(|(k, v)| k == "accept" && v == "*/*"));
    }
    #[test]
    fn manifests_storyboards_and_drm_are_not_playable_format_choices() {
        for (key, value) in [
            ("protocol", json!("m3u8_native")),
            ("has_drm", json!(true)),
            ("manifest_url", json!("https://example.test/manifest")),
            ("fragments", json!([])),
            ("ext", json!("mhtml")),
        ] {
            let mut bad = format();
            bad[key] = value;
            let mut data = video();
            data["formats"] = json!([bad, format()]);
            let parsed = run(data).unwrap();
            assert_eq!(parsed.formats.len(), 1);
            assert_eq!(parsed.rejected_formats, 1);
        }
        let mut data = video();
        data["formats"][0]["protocol"] = json!("http_dash_segments");
        assert_eq!(run(data).unwrap_err(), ExtractError::NoSupportedFormats);
    }
    #[test]
    fn video_identity_playlist_and_metadata_are_checked() {
        let mut data = video();
        data["id"] = json!("12345678901");
        assert_eq!(run(data).unwrap_err(), ExtractError::WrongVideo);
        for key in ["entries", "_type", "has_drm"] {
            let mut data = video();
            data[key] = match key {
                "entries" => json!([]),
                "_type" => json!("playlist"),
                _ => json!(true),
            };
            assert_eq!(run(data).unwrap_err(), ExtractError::UnsupportedVideo);
        }
        for duration in [json!(0), json!(-1), json!(MAX_DURATION + 1.0)] {
            let mut data = video();
            data["duration"] = duration;
            assert_eq!(run(data).unwrap_err(), ExtractError::InvalidMetadata);
        }
    }
    #[test]
    fn format_count_raw_bytes_duplicates_and_malformed_numbers_are_bounded() {
        assert_eq!(
            parse(&vec![b' '; MAX_JSON_BYTES + 1], &id()).unwrap_err(),
            ExtractError::Oversized
        );
        let mut data = video();
        data["formats"] = Value::Array(vec![format(); MAX_FORMATS + 1]);
        assert_eq!(run(data).unwrap_err(), ExtractError::InvalidJson);
        let mut data = video();
        data["formats"] = json!([format(), format()]);
        assert_eq!(run(data).unwrap_err(), ExtractError::InvalidFormat);
        for (key, value) in [
            ("width", json!(0)),
            ("height", json!(20000)),
            ("fps", json!(1000)),
            ("format_id", json!("--exec")),
            ("vcodec", json!("bad\ncodec")),
        ] {
            let mut data = video();
            data["formats"][0][key] = value;
            assert_eq!(run(data).unwrap_err(), ExtractError::InvalidFormat);
        }
    }
    #[test]
    fn duplicate_headers_and_empty_format_sets_are_not_silently_repaired() {
        let mut data = video();
        data["formats"][0]["http_headers"] = json!({"User-Agent":"one","user-agent":"two"});
        assert_eq!(run(data).unwrap_err(), ExtractError::InvalidJson);
        let mut data = video();
        data["formats"] = json!([]);
        assert_eq!(run(data).unwrap_err(), ExtractError::NoSupportedFormats);
        let mut data = video();
        data["formats"][0]["http_headers"] =
            json!({"Referer":id().watch_url(),"Origin":"https://www.youtube.com"});
        assert!(run(data).is_ok());
    }
    #[test]
    fn debug_never_exposes_signed_urls_or_headers() {
        let parsed = run(video()).unwrap();
        for text in [
            format!("{parsed:?}"),
            format!("{:?}", parsed.formats[0]),
            format!("{:?}", parsed.formats[0].url),
            format!("{:?}", parsed.formats[0].headers),
        ] {
            assert!(!text.contains("RAM_ONLY_SECRET"));
            assert!(!text.contains("googlevideo"));
            assert!(!text.contains("fixture"));
        }
    }
    #[test]
    fn unsupported_caption_links_do_not_weaken_or_fail_video_formats() {
        let mut data = video();
        data["subtitles"] = json!({"en":[{"ext":"vtt","url":format!("https://www.youtube.com/api/timedtext?v={}&fmt=vtt",id().as_str()),"name":"English"}],"ru":[{"ext":"vtt","url":"http://localhost/private"}]});
        data["automatic_captions"] = json!({"en":[{"ext":"vtt","url":format!("https://www.youtube.com/api/timedtext?v={}&fmt=vtt",id().as_str())}]});
        let parsed = run(data.clone()).unwrap();
        assert_eq!(parsed.formats.len(), 1);
        assert_eq!(parsed.captions.len(), 2);
        assert!(parsed.rejected_captions > 0);
        assert!(!parsed.captions[0].metadata().automatic);
        assert!(parsed.captions[1].metadata().automatic);
        data["formats"][0]["url"] = json!("https://evil.test/video");
        assert_eq!(run(data).unwrap_err(), ExtractError::UnsafeUrl);
    }
}
