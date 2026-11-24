//! Anonymous routed VTT captions. Signed tracks stay RAM-only; never serialize
//! or log them. Native caller owns operation/document/route generation and drops
//! fetch() on cancellation. Cue text is PLAIN TEXT: consumers use textContent.
use crate::{tab_routes::NativeRoute, youtube_light::VideoId};
use reqwest::Url;
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::{fmt, sync::Arc, time::Duration};
use zeroize::Zeroizing;
pub const MAX_TRACKS: usize = 256;
pub const MAX_BODY: usize = 2 * 1024 * 1024;
pub const MAX_CUES: usize = 10000;
const MAX_TIME: f64 = 604800.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionError {
    InvalidTrack,
    RouteUnavailable,
    Unavailable,
    Oversized,
    InvalidVtt,
}
impl fmt::Display for CaptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Субтитры недоступны или имеют неподдерживаемый формат")
    }
}
impl std::error::Error for CaptionError {}
#[derive(Clone, Serialize)]
pub struct CaptionMetadata {
    pub language: String,
    pub label: String,
    pub automatic: bool,
}
pub struct CaptionTrack {
    metadata: CaptionMetadata,
    video: VideoId,
    url: Zeroizing<String>,
}
impl fmt::Debug for CaptionTrack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CaptionTrack([redacted])")
    }
}
impl CaptionTrack {
    pub fn metadata(&self) -> CaptionMetadata {
        self.metadata.clone()
    }
    pub fn video_id(&self) -> &VideoId {
        &self.video
    }
}
#[derive(Serialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

// Lenient shape parsing means malformed/unsupported caption entries cannot
// weaken video format checks or prevent an otherwise valid video from opening.
// Unknown nested values stream through IgnoredAny instead of allocating Values.
#[derive(Default)]
struct Text(Option<Zeroizing<String>>);
impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("optional bounded text")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Text, E> {
                Ok(Text((s.len() <= 16384).then(|| Zeroizing::new(s.into()))))
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<Text, E> {
                let s = Zeroizing::new(s);
                Ok(Text((s.len() <= 16384).then_some(s)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Text, E> {
                Ok(Text::default())
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Text, E> {
                Ok(Text::default())
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Text, E> {
                Ok(Text::default())
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Text, E> {
                Ok(Text::default())
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Text, E> {
                Ok(Text::default())
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Text, A::Error> {
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(Text::default())
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Text, M::Error> {
                while map
                    .next_entry::<de::IgnoredAny, de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(Text::default())
            }
        }
        d.deserialize_any(V)
    }
}
#[derive(Default)]
struct Candidate {
    language: String,
    ext: Text,
    name: Text,
    url: Text,
}
impl<'de> Deserialize<'de> for Candidate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Candidate;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("caption entry")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Candidate, M::Error> {
                let mut out = Candidate::default();
                let mut seen = std::collections::BTreeSet::new();
                let mut duplicate = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "ext" | "name" | "url" => {
                            duplicate |= !seen.insert(key.clone());
                            let value = map.next_value::<Text>()?;
                            match key.as_str() {
                                "ext" => out.ext = value,
                                "name" => out.name = value,
                                _ => out.url = value,
                            }
                        }
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                if duplicate {
                    out.url = Text::default()
                }
                Ok(out)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Candidate, A::Error> {
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(Candidate::default())
            }
            fn visit_unit<E: de::Error>(self) -> Result<Candidate, E> {
                Ok(Candidate::default())
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Candidate, E> {
                Ok(Candidate::default())
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Candidate, E> {
                Ok(Candidate::default())
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Candidate, E> {
                Ok(Candidate::default())
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Candidate, E> {
                Ok(Candidate::default())
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Candidate, E> {
                Ok(Candidate::default())
            }
        }
        d.deserialize_any(V)
    }
}
#[derive(Default)]
struct Language {
    candidates: Vec<Candidate>,
    rejected: usize,
}
impl<'de> Deserialize<'de> for Language {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Language;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("caption formats")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Language, A::Error> {
                let mut out = Language::default();
                while out.candidates.len() < 32 {
                    match seq.next_element::<Candidate>()? {
                        Some(value) => out.candidates.push(value),
                        None => return Ok(out),
                    }
                }
                while seq.next_element::<de::IgnoredAny>()?.is_some() {
                    out.rejected = out.rejected.saturating_add(1)
                }
                Ok(out)
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Language, M::Error> {
                while map
                    .next_entry::<de::IgnoredAny, de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(Language {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_unit<E: de::Error>(self) -> Result<Language, E> {
                Ok(Language::default())
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Language, E> {
                Ok(Language {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Language, E> {
                Ok(Language {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Language, E> {
                Ok(Language {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Language, E> {
                Ok(Language {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Language, E> {
                Ok(Language {
                    rejected: 1,
                    ..Default::default()
                })
            }
        }
        d.deserialize_any(V)
    }
}
#[derive(Default)]
pub(crate) struct Sources {
    candidates: Vec<Candidate>,
    rejected: usize,
}
impl<'de> Deserialize<'de> for Sources {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Sources;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("caption languages")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Sources, M::Error> {
                let mut out = Sources::default();
                while let Some(language) = map.next_key::<String>()? {
                    if out.candidates.len() >= MAX_TRACKS
                        || language.is_empty()
                        || language.len() > 35
                        || !language
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    {
                        map.next_value::<de::IgnoredAny>()?;
                        out.rejected = out.rejected.saturating_add(1);
                        continue;
                    }
                    let group = map.next_value::<Language>()?;
                    out.rejected = out.rejected.saturating_add(group.rejected);
                    for mut entry in group.candidates {
                        if !entry
                            .ext
                            .0
                            .as_deref()
                            .is_some_and(|ext| ext.as_str() == "vtt")
                            || out.candidates.len() >= MAX_TRACKS
                        {
                            out.rejected = out.rejected.saturating_add(1);
                            continue;
                        }
                        entry.language = language.clone();
                        out.candidates.push(entry);
                    }
                }
                Ok(out)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Sources, A::Error> {
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(Sources {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_unit<E: de::Error>(self) -> Result<Sources, E> {
                Ok(Sources::default())
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Sources, E> {
                Ok(Sources {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Sources, E> {
                Ok(Sources {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Sources, E> {
                Ok(Sources {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Sources, E> {
                Ok(Sources {
                    rejected: 1,
                    ..Default::default()
                })
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Sources, E> {
                Ok(Sources {
                    rejected: 1,
                    ..Default::default()
                })
            }
        }
        d.deserialize_any(V)
    }
}
fn valid_url(input: &str, video: &VideoId) -> bool {
    if input.len() > 16384
        || !input.is_ascii()
        || input.bytes().any(|b| b <= 32 || b == 127 || b == b'\\')
        || !input.starts_with("https://")
    {
        return false;
    }
    let rest = &input[8..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..end].contains(['@', ':', '%']) {
        return false;
    }
    let Ok(url) = Url::parse(input) else {
        return false;
    };
    if url.host_str() != Some("www.youtube.com")
        || url.path() != "/api/timedtext"
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return false;
    }
    let raw = &rest[end..];
    if raw.split('?').next() != Some("/api/timedtext") {
        return false;
    }
    let (mut v, mut fmt) = (None, None);
    for pair in url.query().unwrap_or("").split('&') {
        let (k, value) = pair.split_once('=').unwrap_or((pair, ""));
        if k.contains(['%', '+']) {
            return false;
        }
        if k == "v" {
            if v.replace(value).is_some() {
                return false;
            }
        }
        if k == "fmt" {
            if fmt.replace(value).is_some() {
                return false;
            }
        }
    }
    v == Some(video.as_str()) && fmt == Some("vtt")
}
pub(crate) fn tracks(
    video: &VideoId,
    manual: Sources,
    automatic: Sources,
) -> (Vec<Arc<CaptionTrack>>, usize) {
    let mut out = Vec::new();
    let mut rejected = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for (source, auto) in [(manual, false), (automatic, true)] {
        rejected = rejected.saturating_add(source.rejected);
        for entry in source.candidates {
            let language = &entry.language;
            let label = entry
                .name
                .0
                .as_deref()
                .map(|s| s.as_str())
                .unwrap_or(language.as_str());
            let valid = entry.ext.0.as_deref().is_some_and(|v| v.as_str() == "vtt")
                && language.len() <= 35
                && !language.is_empty()
                && language
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && label.chars().count() <= 80
                && !label.trim().is_empty()
                && !label.chars().any(char::is_control);
            let Some(url) = entry.url.0 else {
                rejected += 1;
                continue;
            };
            if !valid
                || !valid_url(&url, video)
                || out.len() >= MAX_TRACKS
                || !seen.insert((entry.language.clone(), auto))
            {
                rejected += 1;
                continue;
            }
            let label = label.to_owned();
            out.push(Arc::new(CaptionTrack {
                metadata: CaptionMetadata {
                    language: entry.language,
                    label,
                    automatic: auto,
                },
                video: video.clone(),
                url,
            }));
        }
    }
    (out, rejected)
}
/// No retries/cookie jar/redirects/environment proxies. Caller cancels by dropping
/// this future and verifies native route+operation generation before applying.
// Strict extra CA for the fixed owned CI transcript endpoint only.
fn fixture_trust(builder:reqwest::ClientBuilder,track:&CaptionTrack)->Result<reqwest::ClientBuilder,CaptionError>{
    if std::env::var("GITHUB_ACTIONS").as_deref()!=Ok("true")||std::env::var_os("NEXUSBRO_TEST_DATA").is_none()||std::env::var("NEXUSBRO_TEST_YOUTUBE_FEATURES").as_deref()!=Ok("1"){return Ok(builder);}
    let url=reqwest::Url::parse(track.url.as_str()).map_err(|_|CaptionError::Unavailable)?;
    if url.host_str()!=Some("www.youtube.com")||url.path()!="/api/timedtext"||!url.query_pairs().any(|(k,v)|k=="v"&&v=="BaW_jenozKc"){return Ok(builder);}
    let Some(path)=std::env::var_os("NEXUSBRO_TEST_YOUTUBE_CAPTIONS_CA") else{return Ok(builder);};
    let path=std::path::PathBuf::from(path);let metadata=std::fs::symlink_metadata(&path).map_err(|_|CaptionError::Unavailable)?;
    if !path.is_absolute()||!metadata.is_file()||metadata.file_type().is_symlink()||metadata.len()>16384{return Err(CaptionError::Unavailable);}
    let pem=std::fs::read(path).map_err(|_|CaptionError::Unavailable)?;
    let expected=std::env::var("NEXUSBRO_TEST_YOUTUBE_CAPTIONS_CA_SHA256").map_err(|_|CaptionError::Unavailable)?;
    use sha2::Digest;
    if expected.len()!=64||expected!=format!("{:x}",sha2::Sha256::digest(&pem)){return Err(CaptionError::Unavailable);}
    let cert=reqwest::Certificate::from_pem(&pem).map_err(|_|CaptionError::Unavailable)?;
    Ok(builder.add_root_certificate(cert))
}
pub async fn fetch(route: &NativeRoute, track: &CaptionTrack) -> Result<Vec<Cue>, CaptionError> {
    if !route.validate() {
        return Err(CaptionError::RouteUnavailable);
    }
    let proxy = reqwest::Proxy::all(&route.proxy).map_err(|_| CaptionError::RouteUnavailable)?;
    let builder = reqwest::Client::builder()
        .no_proxy()
        .proxy(proxy)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .user_agent("NexusBro/light-youtube");
    let client = fixture_trust(builder,track)?
        .build()
        .map_err(|_| CaptionError::RouteUnavailable)?;
    let mut response = client
        .get(track.url.as_str())
        .header(reqwest::header::ACCEPT, "text/vtt")
        .send()
        .await
        .map_err(|_| CaptionError::Unavailable)?;
    if !response.status().is_success() {
        return Err(CaptionError::Unavailable);
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err(CaptionError::Oversized);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CaptionError::Unavailable)?
    {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|n| n > MAX_BODY)
        {
            return Err(CaptionError::Oversized);
        }
        bytes.extend_from_slice(&chunk)
    }
    parse_vtt(&bytes)
}
fn timestamp(s: &str) -> Option<f64> {
    let parts = s.split(':').collect::<Vec<_>>();
    let (h, m, last) = match parts.as_slice() {
        [m, last] if m.len() == 2 => (0u32, *m, *last),
        [h, m, last]
            if (2..=3).contains(&h.len())
                && m.len() == 2
                && h.bytes().all(|b| b.is_ascii_digit()) =>
        {
            (h.parse().ok()?, *m, *last)
        }
        _ => return None,
    };
    if !m.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let m = m.parse::<u32>().ok()?;
    let (sec, millis) = last.split_once('.')?;
    if sec.len() != 2
        || millis.len() != 3
        || !sec
            .bytes()
            .chain(millis.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let sec = sec.parse::<u32>().ok()?;
    if m >= 60 || sec >= 60 {
        return None;
    }
    let result = (h * 3600 + m * 60 + sec) as f64 + millis.parse::<u32>().ok()? as f64 / 1000.0;
    (result <= MAX_TIME).then_some(result)
}
fn text(input: &str) -> Result<String, CaptionError> {
    let mut output = String::new();
    let mut rest = input;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('<') {
            let (end, remaining) = tail.split_once('>').ok_or(CaptionError::InvalidVtt)?;
            let tag = end.strip_prefix('/').unwrap_or(end);
            let known = matches!(tag, "b" | "i" | "u" | "c" | "v" | "lang" | "ruby" | "rt")
                || tag.strip_prefix("c.").is_some_and(|s| {
                    s.len() <= 80
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                })
                || ["v ", "lang "].iter().any(|prefix| {
                    tag.strip_prefix(prefix).is_some_and(|s| {
                        s.len() <= 160
                            && !s
                                .chars()
                                .any(|c| c.is_control() || matches!(c, '<' | '>' | '&'))
                    })
                })
                || timestamp(tag).is_some();
            if !known {
                return Err(CaptionError::InvalidVtt);
            }
            rest = remaining;
        } else if let Some(tail) = rest.strip_prefix('&') {
            let (entity, remaining) = tail.split_once(';').ok_or(CaptionError::InvalidVtt)?;
            let decoded = match entity {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => ' ',
                "lrm" => '\u{200e}',
                "rlm" => '\u{200f}',
                _ => {
                    let number = entity
                        .strip_prefix("#x")
                        .and_then(|s| u32::from_str_radix(s, 16).ok())
                        .or_else(|| entity.strip_prefix('#').and_then(|s| s.parse().ok()))
                        .ok_or(CaptionError::InvalidVtt)?;
                    char::from_u32(number)
                        .filter(|c| !c.is_control())
                        .ok_or(CaptionError::InvalidVtt)?
                }
            };
            output.push(decoded);
            rest = remaining;
        } else {
            let c = rest.chars().next().unwrap();
            if c.is_control() && !matches!(c, '\n' | '\t') {
                return Err(CaptionError::InvalidVtt);
            }
            output.push(if c == '\t' { ' ' } else { c });
            rest = &rest[c.len_utf8()..];
        }
        if output.len() > 8192 {
            return Err(CaptionError::InvalidVtt);
        }
    }
    if output.chars().count() > 2048 {
        return Err(CaptionError::InvalidVtt);
    }
    Ok(output)
}
pub fn parse_vtt(bytes: &[u8]) -> Result<Vec<Cue>, CaptionError> {
    if bytes.len() > MAX_BODY {
        return Err(CaptionError::Oversized);
    }
    let source = std::str::from_utf8(bytes)
        .map_err(|_| CaptionError::InvalidVtt)?
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    if source.contains('\r') {
        return Err(CaptionError::InvalidVtt);
    }
    let mut blocks = source.split("\n\n");
    let header_block = blocks.next().ok_or(CaptionError::InvalidVtt)?;
    if header_block.lines().any(|line| line.contains("-->")) {
        return Err(CaptionError::InvalidVtt);
    }
    let header = header_block
        .lines()
        .next()
        .ok_or(CaptionError::InvalidVtt)?;
    if header.len() > 512 {
        return Err(CaptionError::InvalidVtt);
    }
    if header != "WEBVTT" && !header.starts_with("WEBVTT ") && !header.starts_with("WEBVTT\t") {
        return Err(CaptionError::InvalidVtt);
    }
    let mut cues = Vec::new();
    let mut total = 0usize;
    let mut previous = 0.0;
    for block in blocks {
        let block = block.trim_start_matches('\n');
        let mut lines = block.lines();
        let Some(first) = lines.next() else { continue };
        if first.is_empty() {
            continue;
        }
        if first == "NOTE" || first.starts_with("NOTE ") || matches!(first, "STYLE" | "REGION") {
            continue;
        }
        let timing = if first.contains("-->") {
            first
        } else {
            lines.next().ok_or(CaptionError::InvalidVtt)?
        };
        if timing.len() > 512 {
            return Err(CaptionError::InvalidVtt);
        }
        let fields = timing.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 || fields[1] != "-->" {
            return Err(CaptionError::InvalidVtt);
        }
        let start = timestamp(fields[0]).ok_or(CaptionError::InvalidVtt)?;
        let end = timestamp(fields[2]).ok_or(CaptionError::InvalidVtt)?;
        if start < previous || end <= start || cues.len() >= MAX_CUES {
            return Err(CaptionError::InvalidVtt);
        }
        let value = text(&lines.collect::<Vec<_>>().join("\n"))?;
        total += value.len();
        if total > 1024 * 1024 {
            return Err(CaptionError::Oversized);
        }
        previous = start;
        cues.push(Cue {
            start,
            end,
            text: value,
        });
    }
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> VideoId {
        VideoId::parse("aB3_dE-9xY0").unwrap()
    }
    fn url() -> String {
        format!(
            "https://www.youtube.com/api/timedtext?v={}&fmt=vtt&sig=PRIVATE",
            id().as_str()
        )
    }
    #[test]
    fn signed_url_is_exact_video_host_path_and_format() {
        assert!(valid_url(&url(), &id()));
        for url in [
            url().replace("www.youtube.com", "www.youtube.com.evil.test"),
            url().replace("www.youtube.com", "www.youtube.com:443"),
            url().replace("https://", "http://"),
            url().replace("/api/", "/x/../api/"),
            url().replace("v=aB3_dE-9xY0", "v=12345678901"),
            format!("{}&v={}", url(), id().as_str()),
            url().replace("fmt=vtt", "fmt=json3"),
        ] {
            assert!(!valid_url(&url, &id()));
        }
    }
    #[test]
    fn malformed_caption_entries_skip_without_authorizing_any_url() {
        let raw = serde_json::json!({"en":[{"ext":"vtt","url":url(),"name":"English"},{"ext":"json3","url":"http://localhost"}],"bad lang":[{"ext":"vtt","url":url()}],"fr":{"oops":true}});
        let source: Sources = serde_json::from_value(raw).unwrap();
        let (tracks, rejected) = tracks(&id(), source, Sources::default());
        assert_eq!(tracks.len(), 1);
        assert!(rejected >= 2);
        assert!(!format!("{:?}", tracks[0]).contains("PRIVATE"));
    }
    #[test]
    fn vtt_becomes_plain_text_with_known_markup_and_entities() {
        let cues=parse_vtt(b"WEBVTT\n\n00:01.000 --> 00:03.500 align:start\n<v Speaker><c.color>Hello &amp; &#x41;</c></v>\n\n00:02.000 --> 00:04.000\n&lt;literal&gt;\n").unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Hello & A");
        assert_eq!(cues[1].text, "<literal>");
    }
    #[test]
    fn malformed_timestamps_markup_entities_and_bounds_fail_closed() {
        for body in [
            "WEBVTT\n\n00:60.000 --> 01:01.000\nx",
            "WEBVTT\n\n00:01.00 --> 00:02.000\nx",
            "WEBVTT\n\n00:02.000 --> 00:01.000\nx",
            "WEBVTT\n\n00:01.000 --> 00:02.000\n<script>x</script>",
            "WEBVTT\n\n00:01.000 --> 00:02.000\n&unknown;",
            "WEBVTT\n\n00:01.000 --> 00:02.000\n&#0;",
        ] {
            assert!(parse_vtt(body.as_bytes()).is_err())
        }
        assert_eq!(
            parse_vtt(&vec![b' '; MAX_BODY + 1]).err(),
            Some(CaptionError::Oversized)
        );
        assert!(timestamp("168:00:00.000").is_some());
        assert!(timestamp("168:00:00.001").is_none());
    }
    #[test]
    fn cue_count_plaintext_and_extra_blank_lines_are_bounded() {
        let cue = "00:01.000 --> 00:02.000\nx\n\n";
        assert!(parse_vtt(format!("WEBVTT\n\n{}", cue.repeat(MAX_CUES + 1)).as_bytes()).is_err());
        let parsed = parse_vtt(
            b"WEBVTT\n\n\nNOTE ignore <script>\nnot executed\n\n00:01.000 --> 00:02.000\nx\n",
        )
        .unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parse_vtt(b"WEBVTT\n00:01.000 --> 00:02.000\nx").is_err());
        assert!(parse_vtt(
            format!("WEBVTT\n\n00:01.000 --> 00:02.000\n{}", "x".repeat(2049)).as_bytes()
        )
        .is_err());
    }
}
