//! Auxiliary YouTube APIs. Every request uses an admitted parent-owned route.
//! Callers must validate tab generation before applying results. No cookies,
//! environment proxies, redirects, direct fallback, or remote error text.
use crate::tab_routes::NativeRoute;
use std::time::Duration;

const MAX_BODY: usize = 512 * 1024;

#[derive(Clone, Copy)]
pub enum AuxiliaryApi {
    SponsorBlock,
    Dislikes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiError {
    InvalidInput,
    RouteUnavailable,
    Unavailable,
    RateLimited,
    Oversized,
    InvalidResponse,
}

fn endpoint(api: AuxiliaryApi, video: &str) -> Result<reqwest::Url, ApiError> {
    if video.len() != 11
        || !video
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(ApiError::InvalidInput);
    }
    let mut url = reqwest::Url::parse(match api {
        AuxiliaryApi::SponsorBlock => "https://sponsor.ajay.app/api/skipSegments",
        AuxiliaryApi::Dislikes => "https://returnyoutubedislikeapi.com/votes",
    })
    .map_err(|_| ApiError::InvalidInput)?;
    match api {
        AuxiliaryApi::SponsorBlock => {
            url.query_pairs_mut()
                .append_pair("videoID", video)
                .append_pair(
                    "categories",
                    r#"["sponsor","selfpromo","interaction","intro","outro"]"#,
                );
        }
        AuxiliaryApi::Dislikes => {
            url.query_pairs_mut().append_pair("videoId", video);
        }
    }
    Ok(url)
}

/// No retry: the session coordinator owns bounded retries and rate limiting.
/// This does not authorize a request: caller must retain the route/session lease.
pub async fn fetch(
    route: &NativeRoute,
    api: AuxiliaryApi,
    video: &crate::youtube_light::VideoId,
) -> Result<serde_json::Value, ApiError> {
    let url = endpoint(api, video.as_str())?;
    if !route.validate() {
        return Err(ApiError::RouteUnavailable);
    }
    let proxy = reqwest::Proxy::all(&route.proxy).map_err(|_| ApiError::RouteUnavailable)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(proxy)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .user_agent("NexusBro/light-youtube")
        .build()
        .map_err(|_| ApiError::RouteUnavailable)?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| ApiError::Unavailable)?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ApiError::RateLimited);
    }
    if matches!(api, AuxiliaryApi::SponsorBlock)
        && response.status() == reqwest::StatusCode::NOT_FOUND
    {
        return Ok(serde_json::json!([]));
    }
    if !response.status().is_success() {
        return Err(ApiError::Unavailable);
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err(ApiError::Oversized);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ApiError::Unavailable)? {
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|n| n > MAX_BODY)
        {
            return Err(ApiError::Oversized);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| ApiError::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_url_and_option_injection() {
        for bad in [
            "https://youtu.be/abcdefghijk",
            "--proxy=foo",
            "../abcdefgh",
            "abcdefghij&",
            "",
            "abcdefghijkl",
        ] {
            assert_eq!(
                endpoint(AuxiliaryApi::Dislikes, bad).unwrap_err(),
                ApiError::InvalidInput
            );
        }
    }
    #[test]
    fn targets_are_fixed_and_video_is_query_data() {
        for api in [AuxiliaryApi::SponsorBlock, AuxiliaryApi::Dislikes] {
            let url = endpoint(api, "abcDE_12-34").unwrap();
            assert_eq!(url.scheme(), "https");
            assert!(matches!(
                url.host_str(),
                Some("sponsor.ajay.app" | "returnyoutubedislikeapi.com")
            ));
            assert_eq!(url.query_pairs().next().unwrap().1, "abcDE_12-34");
            assert!(url.username().is_empty());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkipSegment {
    pub start: f64,
    pub end: f64,
}

/// API data is untrusted. Never seek from arbitrary JSON or caller categories.
/// Unsupported actions/categories are ignored; malformed supported data fails.
pub fn skip_segments(
    value: &serde_json::Value,
    duration: f64,
) -> Result<Vec<SkipSegment>, ApiError> {
    if !duration.is_finite() || duration <= 0.0 || duration > 7.0 * 86400.0 {
        return Err(ApiError::InvalidInput);
    }
    let rows = value.as_array().ok_or(ApiError::InvalidResponse)?;
    if rows.len() > 500 {
        return Err(ApiError::Oversized);
    }
    let mut segments = Vec::new();
    for row in rows {
        if row.get("actionType").and_then(|v| v.as_str()) != Some("skip") {
            continue;
        }
        if !matches!(
            row.get("category").and_then(|v| v.as_str()),
            Some("sponsor" | "selfpromo" | "interaction" | "intro" | "outro")
        ) {
            continue;
        }
        let pair = row
            .get("segment")
            .and_then(|v| v.as_array())
            .filter(|v| v.len() == 2)
            .ok_or(ApiError::InvalidResponse)?;
        let start = pair[0].as_f64().ok_or(ApiError::InvalidResponse)?;
        let end = pair[1].as_f64().ok_or(ApiError::InvalidResponse)?;
        if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start || end > duration {
            return Err(ApiError::InvalidResponse);
        }
        segments.push(SkipSegment { start, end });
    }
    segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut merged: Vec<SkipSegment> = Vec::new();
    for segment in segments {
        if let Some(last) = merged.last_mut().filter(|last| segment.start <= last.end) {
            last.end = last.end.max(segment.end);
        } else {
            merged.push(segment);
        }
    }
    Ok(merged)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DislikeEstimate {
    pub likes: u64,
    pub dislikes: u64,
}
/// Counts are third-party estimates, never labeled exact YouTube statistics.
pub fn dislike_estimate(value: &serde_json::Value) -> Result<DislikeEstimate, ApiError> {
    let count = |key| {
        value
            .get(key)
            .and_then(|v| v.as_u64())
            .filter(|n| *n <= (1u64 << 53) - 1)
            .ok_or(ApiError::InvalidResponse)
    };
    Ok(DislikeEstimate {
        likes: count("likes")?,
        dislikes: count("dislikes")?,
    })
}

#[cfg(test)]
mod response_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn malicious_segment_never_becomes_a_seek() {
        for pair in [
            json!([-1, 20]),
            json!([20, 10]),
            json!([10, 101]),
            json!(["10", 20]),
            json!([10, 20, 30]),
        ] {
            assert!(skip_segments(
                &json!([{"actionType":"skip","category":"sponsor","segment":pair}]),
                100.0
            )
            .is_err());
        }
    }
    #[test]
    fn skips_only_supported_actions_and_merges_overlaps() {
        let input = json!([
            {"actionType":"skip","category":"sponsor","segment":[15,25]},
            {"actionType":"skip","category":"sponsor","segment":[10,20]},
            {"actionType":"full","category":"sponsor","segment":[0,100]},
            {"actionType":"skip","category":"unknown","segment":[0,100]}
        ]);
        assert_eq!(
            skip_segments(&input, 100.0).unwrap(),
            vec![SkipSegment {
                start: 10.0,
                end: 25.0
            }]
        );
    }
    #[test]
    fn dislike_counts_reject_negative_fractional_and_unbounded_values() {
        assert_eq!(
            dislike_estimate(&json!({"likes":3,"dislikes":2})).unwrap(),
            DislikeEstimate {
                likes: 3,
                dislikes: 2
            }
        );
        for bad in [json!(-1), json!(0.5), json!(1u64 << 53), json!("2")] {
            assert!(dislike_estimate(&json!({"likes":3,"dislikes":bad})).is_err());
        }
    }
}
