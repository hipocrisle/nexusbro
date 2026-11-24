//! CI-only real invocation of production extractor/parser. No CEF playback claim.
use nexusbro_youtube_light_verification::{
    tab_routes::NativeRoute,
    youtube_light::VideoId,
    youtube_light_extract,
    youtube_light_sidecar::{self, Components},
};
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") || args.len() != 3 {
        std::process::exit(2);
    }
    // The CI wrapper assigns the Windows Job BEFORE any helper can spawn.
    // Linux wrapper establishes a new process group before this handshake.
    use std::io::{BufRead, Read};
    let mut admission = String::new();
    if std::io::BufReader::new(std::io::stdin().take(4))
        .read_line(&mut admission)
        .is_err()
        || admission != "GO\n"
    {
        std::process::exit(2);
    }
    let directory = std::path::Path::new(&args[0]);
    let Some(video) = VideoId::parse(&args[2]).ok() else {
        std::process::exit(2);
    };
    let components = Components::new(
        directory.join(if cfg!(windows) {
            "yt-dlp.exe"
        } else {
            "yt-dlp"
        }),
        directory.join(if cfg!(windows) { "deno.exe" } else { "deno" }),
    );
    let Ok(components) = components else {
        println!("{}", serde_json::json!({"stage":"components","ok":false}));
        std::process::exit(3);
    };
    let route = NativeRoute {
        key: "a".repeat(64),
        proxy: args[1].clone(),
        allow_non_proxied_udp: false,
    };
    let (_tx, rx) = tokio::sync::watch::channel(false);
    match youtube_light_sidecar::extract_diagnosed(&components, &video, &route, rx).await {
        Ok(bytes) => match youtube_light_extract::parse(&bytes, &video) {
            Ok(data) => {
                let vp9 = data
                    .formats
                    .iter()
                    .filter(|f| {
                        f.ext == "webm"
                            && f.vcodec
                                .as_deref()
                                .is_some_and(|v| v.starts_with("vp9") || v.starts_with("vp09"))
                    })
                    .count();
                let opus = data
                    .formats
                    .iter()
                    .filter(|f| f.ext == "webm" && f.acodec.as_deref() == Some("opus"))
                    .count();
                let ready = vp9 > 0 && opus > 0;
                println!(
                    "{}",
                    serde_json::json!({"stage":"parsed","ok":ready,"formats":data.formats.len(),"vp9":vp9,"opus":opus,"rejected_formats":data.rejected_formats})
                );
                if !ready {
                    std::process::exit(5);
                }
            }
            Err(error) => {
                println!(
                    "{}",
                    serde_json::json!({"stage":"parser","ok":false,"reason":error})
                );
                std::process::exit(4);
            }
        },
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({"stage":"extractor","ok":false,"reason":format!("{:?}",error.error),"diagnostic":error.diagnostic})
            );
            std::process::exit(3);
        }
    }
}
