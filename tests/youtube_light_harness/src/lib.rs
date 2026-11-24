//! CI-only source harness: no CEF or desktop build, not playback acceptance.
#[path = "../../../desktop/src-tauri/src/tab_routes.rs"]
pub mod tab_routes;
#[path = "../../../desktop/src-tauri/src/youtube_light.rs"]
pub mod youtube_light;
#[path = "../../../desktop/src-tauri/src/youtube_light_extract.rs"]
pub mod youtube_light_extract;
#[path = "../../../desktop/src-tauri/src/youtube_light_http.rs"]
pub mod youtube_light_http;
#[path = "../../../desktop/src-tauri/src/youtube_light_sidecar.rs"]
pub mod youtube_light_sidecar;

#[cfg(test)]
mod transport_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn auxiliary_api_uses_the_selected_transport_and_reports_its_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let witness = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0u8; 4096];
            let size = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..size]);
            assert!(request.starts_with("CONNECT returnyoutubedislikeapi.com:443 HTTP/1.1\r\n"));
            socket
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let route = tab_routes::NativeRoute {
            key: "a".repeat(64),
            proxy: format!("http://127.0.0.1:{port}"),
            allow_non_proxied_udp: false,
        };
        let id = youtube_light::VideoId::parse("abcDE_12-34").unwrap();
        let answer =
            youtube_light_http::fetch(&route, youtube_light_http::AuxiliaryApi::Dislikes, &id)
                .await;
        assert_eq!(
            answer.unwrap_err(),
            youtube_light_http::ApiError::Unavailable
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), witness)
            .await
            .unwrap()
            .unwrap();
    }
}

#[path = "../../../desktop/src-tauri/src/youtube_light_jobs.rs"]
pub mod youtube_light_jobs;

#[path = "../../../desktop/src-tauri/src/youtube_light_resume.rs"]
pub mod youtube_light_resume;

#[path = "../../../desktop/src-tauri/src/youtube_light_resume_service.rs"]
pub mod youtube_light_resume_service;
#[path = "../../../desktop/src-tauri/src/youtube_light_aux.rs"]
pub mod youtube_light_aux;

#[path = "../../../desktop/src-tauri/src/youtube_light_captions.rs"]
pub mod youtube_light_captions;

#[path = "../../../desktop/src-tauri/src/youtube_light_components.rs"]
pub mod youtube_light_components;
#[path = "../../../desktop/src-tauri/src/youtube_light_download.rs"]
pub mod youtube_light_download;
#[path = "../../../desktop/src-tauri/src/youtube_light_mux.rs"]
pub mod youtube_light_mux;

#[path = "../../../desktop/src-tauri/src/youtube_light_reload.rs"]
pub mod youtube_light_reload;
