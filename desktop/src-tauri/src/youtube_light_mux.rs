//! Verified FFmpeg local stream-copy only. Run on a contained CEF job worker,
//! never CEF UI. Root retains sleep protection until success or proven cleanup.
use crate::youtube_light_components::MuxComponents;
use std::{
    fmt,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::watch,
};
use zeroize::Zeroizing;
const TIMEOUT: Duration = Duration::from_secs(1800);
const REAP: Duration = Duration::from_secs(5);
const PIPE_LIMIT: usize = 4 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuxError {
    InvalidInput,
    Components,
    Spawn,
    Read,
    OutputLimit,
    Failed,
    Cancelled,
    TimedOut,
    CleanupUnknown,
}
impl fmt::Display for MuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CleanupUnknown => "Завершение обработки медиа не подтверждено",
            _ => "Не удалось безопасно объединить локальные потоки медиа",
        })
    }
}
impl std::error::Error for MuxError {}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub bytes: u64,
    pub position_us: u64,
}
fn regular(path: &Path) -> bool {
    path.is_absolute()
        && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}
fn command(
    components: &MuxComponents,
    video: &Path,
    audio: Option<&Path>,
    output: &Path,
) -> Result<Command, MuxError> {
    components.recheck().map_err(|_| MuxError::Components)?;
    if !regular(video)
        || audio.is_some_and(|a| !regular(a))
        || !output.is_absolute()
        || output.exists()
        || output.symlink_metadata().is_ok()
    {
        return Err(MuxError::InvalidInput);
    }
    let parent = output.parent().ok_or(MuxError::InvalidInput)?;
    if !std::fs::symlink_metadata(parent).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        || video.parent() != Some(parent)
        || audio.is_some_and(|a| a.parent() != Some(parent))
    {
        return Err(MuxError::InvalidInput);
    }
    let mut c = Command::new(components.ffmpeg());
    c.args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-nostats",
        "-n",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "matroska",
        "-i",
    ])
    .arg(video);
    if let Some(audio) = audio {
        c.args(["-protocol_whitelist", "file,pipe", "-f", "matroska", "-i"])
            .arg(audio)
            .args(["-map", "0:v:0", "-map", "1:a:0"]);
    } else {
        c.args(["-map", "0:v:0", "-map", "0:a:0"]);
    }
    c.args([
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "-c",
        "copy",
        "-f",
        "webm",
        "-progress",
        "pipe:1",
    ])
    .arg(output);
    c.env_clear();
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            c.env(key, value);
        }
    }
    #[cfg(windows)]
    {
        c.env("PATH", components.bin());
        c.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    {
        c.env("LD_LIBRARY_PATH", components.lib());
    }
    c.current_dir(components.bin())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(c)
}
fn progress_line(line: &[u8], progress: &Mutex<Progress>) -> Result<(), MuxError> {
    if line.len() > 1024 {
        return Err(MuxError::OutputLimit);
    }
    let Some(equal) = line.iter().position(|b| *b == b'=') else {
        return Ok(());
    };
    let (key, value) = (&line[..equal], &line[equal + 1..]);
    if !matches!(key, b"total_size" | b"out_time_us") {
        return Ok(());
    }
    if value == b"N/A" {
        return Ok(());
    }
    if value.is_empty() || value.len() > 20 || !value.iter().all(u8::is_ascii_digit) {
        return Err(MuxError::Read);
    }
    let value = std::str::from_utf8(value)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or(MuxError::Read)?;
    let mut p = progress.lock().map_err(|_| MuxError::Read)?;
    if key == b"total_size" {
        p.bytes = value;
    } else {
        p.position_us = value;
    }
    Ok(())
}
async fn pipe<R: AsyncRead + Unpin>(
    mut source: R,
    progress: Option<&Mutex<Progress>>,
) -> Result<(), MuxError> {
    let mut chunk = Zeroizing::new([0u8; 4096]);
    let mut line = Zeroizing::new(Vec::new());
    let mut total = 0usize;
    loop {
        let n = source.read(&mut *chunk).await.map_err(|_| MuxError::Read)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n).ok_or(MuxError::OutputLimit)?;
        if total > PIPE_LIMIT {
            return Err(MuxError::OutputLimit);
        }
        if let Some(progress) = progress {
            for &byte in &chunk[..n] {
                if byte == b'\n' {
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    progress_line(&line, progress)?;
                    line.clear();
                } else {
                    if line.len() >= 1024 {
                        return Err(MuxError::OutputLimit);
                    }
                    line.push(byte);
                }
            }
        }
    }
    if let Some(progress) = progress {
        if !line.is_empty() {
            progress_line(&line, progress)?;
        }
    }
    Ok(())
}
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}
/// Root must await completion and cancel through watch. A dropped future uses
/// kill_on_drop but is NOT proof of cleanup; parent must latch that outcome.
pub async fn mux(
    components: &MuxComponents,
    video: &Path,
    audio: Option<&Path>,
    output: &Path,
    mut cancel: watch::Receiver<bool>,
    mut progress: impl FnMut(Progress) -> bool,
) -> Result<(), MuxError> {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(MuxError::Cancelled);
    }
    let mut command = command(components, video, audio, output)?;
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(MuxError::Cancelled);
    }
    let mut child = command.spawn().map_err(|_| MuxError::Spawn)?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.start_kill();
        return match tokio::time::timeout(REAP, child.wait()).await {
            Ok(Ok(_)) => Err(MuxError::Read),
            _ => Err(MuxError::CleanupUnknown),
        };
    };
    let counters = Arc::new(Mutex::new(Progress::default()));
    let result = {
        let work = async {
            tokio::try_join!(pipe(stdout, Some(&counters)), pipe(stderr, None))?;
            let status = child.wait().await.map_err(|_| MuxError::CleanupUnknown)?;
            if !status.success() {
                return Err(MuxError::Failed);
            }
            Ok(())
        };
        tokio::pin!(work);
        let timeout = tokio::time::sleep(TIMEOUT);
        tokio::pin!(timeout);
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {biased;_=cancelled(&mut cancel)=>break Err(MuxError::Cancelled),_= &mut timeout=>break Err(MuxError::TimedOut),value=&mut work=>break value,_=tick.tick()=>{let value=match counters.lock(){Ok(v)=>*v,Err(_)=>break Err(MuxError::Read)};if !progress(value){break Err(MuxError::Cancelled);}}}
        }
    };
    if result.is_err() {
        let _ = child.start_kill();
        if !matches!(tokio::time::timeout(REAP, child.wait()).await, Ok(Ok(_))) {
            return Err(MuxError::CleanupUnknown);
        }
    }
    if result.is_ok() {
        let value = *counters.lock().map_err(|_| MuxError::Read)?;
        if !progress(value) {
            return Err(MuxError::Cancelled);
        }
    }
    result?;
    verify_output(components, output, cancel).await
}
async fn probe_bytes<R: AsyncRead + Unpin>(mut source: R) -> Result<Zeroizing<Vec<u8>>, MuxError> {
    let mut result = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 4096]);
    loop {
        let n = source.read(&mut *chunk).await.map_err(|_| MuxError::Read)?;
        if n == 0 {
            break;
        }
        if result.len() + n > 65536 {
            return Err(MuxError::OutputLimit);
        }
        result.extend_from_slice(&chunk[..n]);
    }
    Ok(result)
}
fn validate_probe(bytes: &[u8]) -> Result<(), MuxError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| MuxError::Failed)?;
    let streams = value["streams"]
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or(MuxError::Failed)?;
    let mut video = 0;
    let mut audio = 0;
    for stream in streams {
        match (stream["codec_type"].as_str(), stream["codec_name"].as_str()) {
            (Some("video"), Some("vp9")) => {
                if !["width", "height"]
                    .iter()
                    .all(|key| stream[*key].as_u64().is_some_and(|n| n > 0 && n <= 16384))
                {
                    return Err(MuxError::Failed);
                }
                video += 1;
            }
            (Some("audio"), Some("opus")) => audio += 1,
            _ => return Err(MuxError::Failed),
        }
    }
    if video != 1 || audio != 1 {
        return Err(MuxError::Failed);
    }
    Ok(())
}
async fn verify_output(
    components: &MuxComponents,
    output: &Path,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), MuxError> {
    let Some(executable) = components.ffprobe() else {
        return Ok(());
    };
    components.recheck().map_err(|_| MuxError::Components)?;
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Err(MuxError::Cancelled);
    }
    let mut c = Command::new(executable);
    c.args([
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "matroska",
        "-i",
    ])
    .arg(output)
    .args([
        "-show_entries",
        "stream=codec_type,codec_name,width,height",
        "-of",
        "json",
    ]);
    c.env_clear();
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            c.env(key, value);
        }
    }
    #[cfg(windows)]
    {
        c.env("PATH", components.bin());
        c.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    {
        c.env("LD_LIBRARY_PATH", components.lib());
    }
    c.current_dir(components.bin())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = c.spawn().map_err(|_| MuxError::Spawn)?;
    let pipes = (child.stdout.take(), child.stderr.take());
    let result = if let (Some(out), Some(err)) = pipes {
        let work = async {
            let (stdout, _stderr) = tokio::try_join!(probe_bytes(out), probe_bytes(err))?;
            let status = child.wait().await.map_err(|_| MuxError::CleanupUnknown)?;
            if !status.success() {
                return Err(MuxError::Failed);
            }
            validate_probe(&stdout)
        };
        tokio::select! {biased;_=cancelled(&mut cancel)=>Err(MuxError::Cancelled),_=tokio::time::sleep(Duration::from_secs(20))=>Err(MuxError::TimedOut),result=work=>result}
    } else {
        Err(MuxError::Read)
    };
    if result.is_err() {
        let _ = child.start_kill();
        if !matches!(tokio::time::timeout(REAP, child.wait()).await, Ok(Ok(_))) {
            return Err(MuxError::CleanupUnknown);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_probe_must_report_exact_vp9_and_opus_streams() {
        let good=br#"{"streams":[{"codec_type":"video","codec_name":"vp9","width":1920,"height":1080},{"codec_type":"audio","codec_name":"opus"}]}"#;
        validate_probe(good).unwrap();
        for bad in [br#"{"streams":[]}"#.as_slice(),br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080},{"codec_type":"audio","codec_name":"opus"}]}"#.as_slice(),br#"{"streams":[{"codec_type":"audio","codec_name":"opus"},{"codec_type":"audio","codec_name":"opus"}]}"#.as_slice()]{assert!(validate_probe(bad).is_err());}
    }
    #[test]
    fn progress_only_accepts_bounded_numeric_fields() {
        let counters = Mutex::new(Progress::default());
        progress_line(b"total_size=123", &counters).unwrap();
        progress_line(b"out_time_us=456", &counters).unwrap();
        assert_eq!(counters.lock().unwrap().bytes, 123);
        assert_eq!(counters.lock().unwrap().position_us, 456);
        progress_line(b"unknown=https://secret", &counters).unwrap();
        assert!(progress_line(b"total_size=-1", &counters).is_err());
        assert!(progress_line(b"total_size=18446744073709551616", &counters).is_err());
        assert!(progress_line(&vec![b'x'; 1025], &counters).is_err());
    }
    #[tokio::test]
    async fn pipe_rejects_oversized_lines_without_exposing_stderr() {
        let input = vec![b'x'; 1025];
        assert_eq!(
            pipe(input.as_slice(), Some(&Mutex::new(Progress::default())))
                .await
                .unwrap_err(),
            MuxError::OutputLimit
        );
        assert!(pipe(b"secret raw error".as_slice(), None).await.is_ok());
    }
}
