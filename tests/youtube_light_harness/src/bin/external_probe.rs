//! Explicitly admitted test invocation of production extractor/parser. No playback claim.
#[cfg(feature = "encrypted-diagnostics")]
#[path = "../diagnostic_capture.rs"]
mod diagnostic_capture;
use nexusbro_youtube_light_verification::{
    tab_routes::NativeRoute,
    youtube_light::VideoId,
    youtube_light_extract,
    youtube_light_sidecar::{self, Components},
};
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let raw: Vec<_> = std::env::args().skip(1).collect();
    let consumer = raw.first().is_some_and(|v| v == "--consumer-marker");
    let args = if consumer {
        #[cfg(all(feature = "consumer-probe", target_os = "linux"))]
        {
            match local_admission::admit(&raw) {
                Ok(v) => v,
                Err(code) => {
                    println!(
                        "{}",
                        serde_json::json!({"stage":"admission","ok":false,"reason":code})
                    );
                    std::process::exit(2);
                }
            }
        }
        #[cfg(not(all(feature = "consumer-probe", target_os = "linux")))]
        {
            println!(
                "{}",
                serde_json::json!({"stage":"admission","ok":false,"reason":"consumer_feature_or_platform_unavailable"})
            );
            std::process::exit(2);
        }
    } else {
        raw
    };
    if (!consumer && std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")) || args.len() != 3 {
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
    let experiment =
        !consumer && std::env::var("NEXUSBRO_TEST_IMPERSONATE_CHROME").as_deref() == Ok("1");
    let mut capability = None;
    let result = if experiment {
        youtube_light_sidecar::extract_chrome_experiment(
            &components,
            &video,
            &route,
            rx,
            &mut capability,
        )
        .await
    } else {
        youtube_light_sidecar::extract_diagnosed(&components, &video, &route, rx).await
    };
    match result {
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
                    serde_json::json!({"stage":"parsed","impersonate_chrome":experiment,"chrome_available":capability,"ok":ready,"formats":data.formats.len(),"vp9":vp9,"opus":opus,"rejected_formats":data.rejected_formats})
                );
                if !ready {
                    std::process::exit(5);
                }
            }
            Err(error) => {
                println!(
                    "{}",
                    serde_json::json!({"stage":"parser","impersonate_chrome":experiment,"chrome_available":capability,"ok":false,"reason":error})
                );
                std::process::exit(4);
            }
        },
        Err(error) => {
            #[cfg(feature = "encrypted-diagnostics")]
            let _ = diagnostic_capture::write_failure(&error);
            #[cfg(feature = "encrypted-diagnostics")]
            let captured = error.ci_stderr();
            #[cfg(not(feature = "encrypted-diagnostics"))]
            let captured: Option<&[u8]> = None;
            let flags = captured.map(challenge_subreason).unwrap_or([false; 4]);
            let subreason = serde_json::json!({
                "available": captured.is_some(),
                "bot_confirmation": flags[0], "js_challenge": flags[1],
                "challenge_solving_failed": flags[2], "po_token_required": flags[3]
            });
            println!(
                "{}",
                serde_json::json!({"challenge_subreason":subreason,"stage":"extractor","impersonate_chrome":experiment,"chrome_available":capability,"ok":false,"reason":format!("{:?}",error.error),"diagnostic":error.diagnostic})
            );
            // process::exit skips Drop; wipe retained CI stderr explicitly.
            drop(error);
            std::process::exit(3);
        }
    }
}

/// Fixed diagnostic predicates only; never copy remote text into a summary.
fn challenge_subreason(stderr: &[u8]) -> [bool; 4] {
    let has = |needle: &[u8]| {
        stderr
            .windows(needle.len())
            .any(|part| part.eq_ignore_ascii_case(needle))
    };
    [
        has(b"sign in to confirm you") && has(b"not a bot"),
        has(b"js challenge") || has(b"javascript challenge"),
        has(b"challenge solving failed"),
        has(b"po token is required") || has(b"requires a po token"),
    ]
}

#[cfg(test)]
mod diagnostic_capture_challenge_tests {
    use super::challenge_subreason;
    #[test]
    fn fixed_classes_are_independent_and_case_insensitive() {
        for (text, expected) in [
            (
                b"Sign in to confirm you're not a bot".as_slice(),
                [true, false, false, false],
            ),
            (
                b"JAVASCRIPT CHALLENGE".as_slice(),
                [false, true, false, false],
            ),
            (
                b"n challenge solving failed".as_slice(),
                [false, false, true, false],
            ),
            (
                b"requires a PO token".as_slice(),
                [false, false, false, true],
            ),
            (
                b"WARNING: unknown https://private.invalid/secret".as_slice(),
                [false; 4],
            ),
            (b"sign in to confirm your age".as_slice(), [false; 4]),
            (b"".as_slice(), [false; 4]),
        ] {
            assert_eq!(challenge_subreason(text), expected);
        }
    }
}

/// This feature is a test harness gate, never an application permission. It
/// intentionally refuses root, GHA spoofing, mutable runtime pins and replay.
#[cfg(all(feature = "consumer-probe", target_os = "linux"))]
mod local_admission {
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::{
        ffi::{CStr, OsStr},
        fs::{self, File, OpenOptions},
        io::Read,
        os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };
    type Gate<T> = Result<T, &'static str>;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Marker {
        schema: u32,
        purpose: String,
        uid: u32,
        video: String,
        expires_unix: u64,
    }
    fn seal(value: Option<&str>) -> Gate<&str> {
        value
            .filter(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or("consumer_build_seal_missing")
    }
    fn now() -> Gate<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_secs())
            .map_err(|_| "consumer_clock_invalid")
    }
    fn check_marker(m: &Marker, uid: u32, video: &str, time: u64) -> Gate<()> {
        if uid == 0 || m.uid != uid {
            return Err("consumer_uid_refused");
        }
        if m.schema != 1 || m.purpose != "youtube-anonymous-consumer-probe" || m.video != video {
            return Err("consumer_marker_refused");
        }
        if m.expires_unix <= time || m.expires_unix - time > 600 {
            return Err("consumer_marker_expired");
        }
        Ok(())
    }
    fn private_dir(path: &Path, uid: u32) -> Gate<()> {
        let meta = fs::symlink_metadata(path).map_err(|_| "consumer_directory_refused")?;
        if !meta.file_type().is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err("consumer_directory_refused");
        }
        Ok(())
    }
    fn file(path: &Path, uid: u32, max: u64, marker: bool) -> Gate<File> {
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| "consumer_file_refused")?;
        let m = f.metadata().map_err(|_| "consumer_file_refused")?;
        if !m.file_type().is_file()
            || m.uid() != uid
            || m.len() == 0
            || m.len() > max
            || m.mode() & 0o6022 != 0
            || (marker && m.mode() & 0o777 != 0o600)
        {
            return Err("consumer_file_refused");
        }
        Ok(f)
    }
    fn read_marker(path: &Path, uid: u32) -> Gate<Marker> {
        let mut f = file(path, uid, 4096, true)?;
        let mut bytes = Vec::new();
        f.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| "consumer_marker_refused")?;
        if bytes.len() > 4096 {
            return Err("consumer_marker_refused");
        }
        serde_json::from_slice(&bytes).map_err(|_| "consumer_marker_refused")
    }
    fn passwd_home(uid: u32) -> Gate<PathBuf> {
        // Kernel identity, not caller-controlled HOME/USER variables.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        let mut storage = vec![0u8; 65536];
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut pwd,
                storage.as_mut_ptr().cast(),
                storage.len(),
                &mut result,
            )
        };
        if status != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return Err("consumer_home_unavailable");
        }
        let bytes = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_bytes();
        Ok(PathBuf::from(OsStr::from_bytes(bytes)))
    }
    fn verify_bundle(directory: &Path, uid: u32, expected: &str) -> Gate<()> {
        private_dir(directory, uid)?;
        let mut f = file(&directory.join("components.json"), uid, 131072, false)?;
        let mut bytes = Vec::new();
        f.take(131073)
            .read_to_end(&mut bytes)
            .map_err(|_| "consumer_manifest_refused")?;
        if bytes.len() > 131072 || format!("{:x}", Sha256::digest(&bytes)) != expected {
            return Err("consumer_manifest_refused");
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "consumer_manifest_refused")?;
        if manifest["schema"] != 1 || manifest["platform"] != "linux-x64" {
            return Err("consumer_manifest_refused");
        }
        for name in ["yt-dlp", "deno"] {
            let record = &manifest["components"][name];
            if record["filename"].as_str() != Some(name) {
                return Err("consumer_component_refused");
            }
            let pin = record["sha256"]
                .as_str()
                .ok_or("consumer_component_refused")?;
            seal(Some(pin)).map_err(|_| "consumer_component_refused")?;
            let mut input = file(&directory.join(name), uid, 256 * 1024 * 1024, false)?;
            if input
                .metadata()
                .map_err(|_| "consumer_component_refused")?
                .mode()
                & 0o100
                == 0
            {
                return Err("consumer_component_refused");
            }
            let mut digest = Sha256::new();
            let mut block = [0u8; 65536];
            let mut count = 0u64;
            loop {
                let n = input
                    .read(&mut block)
                    .map_err(|_| "consumer_component_refused")?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                if count > 256 * 1024 * 1024 {
                    return Err("consumer_component_refused");
                }
                digest.update(&block[..n]);
            }
            if format!("{:x}", digest.finalize()) != pin {
                return Err("consumer_component_refused");
            }
        }
        Ok(())
    }
    pub fn admit(args: &[String]) -> Gate<Vec<String>> {
        if args.len() != 5 || std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
            return Err("consumer_mode_refused");
        }
        let expected = seal(option_env!("NEXUSBRO_CONSUMER_MANIFEST_SHA256"))?;
        let uid = unsafe { libc::geteuid() };
        if uid == 0 || uid != unsafe { libc::getuid() } {
            return Err("consumer_uid_refused");
        }
        let marker = Path::new(&args[1]);
        if !marker.is_absolute() {
            return Err("consumer_directory_refused");
        }
        let root = marker.parent().ok_or("consumer_directory_refused")?;
        private_dir(root, uid)?;
        let canonical = root
            .canonicalize()
            .map_err(|_| "consumer_directory_refused")?;
        let ai = passwd_home(uid)?
            .join("AI")
            .canonicalize()
            .map_err(|_| "consumer_home_unavailable")?;
        if canonical == ai || !canonical.starts_with(&ai) {
            return Err("consumer_directory_refused");
        }
        let components = Path::new(&args[2]);
        let resolved = components
            .canonicalize()
            .map_err(|_| "consumer_directory_refused")?;
        let executable = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|_| "consumer_executable_refused")?;
        if !components.is_absolute()
            || !resolved.starts_with(&canonical)
            || !executable.starts_with(&canonical)
        {
            return Err("consumer_directory_refused");
        }
        private_dir(components, uid)?;
        file(&executable, uid, 256 * 1024 * 1024, false)?;
        let video = super::VideoId::parse(&args[4]).map_err(|_| "consumer_video_refused")?;
        let route = super::NativeRoute {
            key: "a".repeat(64),
            proxy: args[3].clone(),
            allow_non_proxied_udp: false,
        };
        if !route.validate() {
            return Err("consumer_route_refused");
        }
        let admission = read_marker(marker, uid)?;
        check_marker(&admission, uid, video.as_str(), now()?)?;
        verify_bundle(components, uid, expected)?;
        check_marker(&admission, uid, video.as_str(), now()?)?;
        // Atomic unlink success is the single-use receipt: parallel/replayed
        // readers cannot both proceed. No helper has spawned at this point.
        fs::remove_file(marker).map_err(|_| "consumer_marker_consumed")?;
        Ok(args[2..].to_vec())
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        fn marker() -> Marker {
            Marker {
                schema: 1,
                purpose: "youtube-anonymous-consumer-probe".into(),
                uid: 1001,
                video: "aqz-KE-bpKQ".into(),
                expires_unix: 1200,
            }
        }
        #[test]
        fn missing_or_runtime_manifest_pin_never_admits() {
            assert_eq!(seal(None).unwrap_err(), "consumer_build_seal_missing");
            assert!(seal(Some(&"x".repeat(64))).is_err());
        }
        #[test]
        fn stale_wrong_uid_wrong_video_and_unknown_fields_refuse_without_helpers() {
            let m = marker();
            assert!(check_marker(&m, 1001, "aqz-KE-bpKQ", 1000).is_ok());
            assert!(check_marker(&m, 0, "aqz-KE-bpKQ", 1000).is_err());
            assert!(check_marker(&m, 1002, "aqz-KE-bpKQ", 1000).is_err());
            assert!(check_marker(&m, 1001, "jNQXAC9IVRw", 1000).is_err());
            assert!(check_marker(&m, 1001, "aqz-KE-bpKQ", 1200).is_err());
            assert!(check_marker(&m, 1001, "aqz-KE-bpKQ", 500).is_err());
            assert!(serde_json::from_str::<Marker>(r#"{"schema":1,"purpose":"youtube-anonymous-consumer-probe","uid":1001,"video":"aqz-KE-bpKQ","expires_unix":1200,"extra":"not admitted"}"#).is_err());
        }
        #[test]
        fn symlink_marker_refuses_before_reading_target() {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let dir = std::env::temp_dir().join(format!(
                "nb-consumer-marker-{}-{}",
                std::process::id(),
                now().unwrap()
            ));
            fs::create_dir(&dir).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            let target = dir.join("target");
            fs::write(&target, b"not a marker").unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
            let link = dir.join("link");
            symlink(&target, &link).unwrap();
            assert!(read_marker(&link, unsafe { libc::geteuid() }).is_err());
            fs::remove_dir_all(dir).unwrap();
        }
    }
}
