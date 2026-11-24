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
