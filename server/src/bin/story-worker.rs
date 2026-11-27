//! The stories worker: a small separate container with ffmpeg. It claims one job at a
//! time from the chat server's internal API (`/internal/jobs/...`, bearer
//! `WORKER_TOKEN`), downloads the input, runs ffprobe/ffmpeg at low priority, uploads
//! the outputs and reports the result. No database, no volume: only `/tmp/job-{id}`.
//!
//! - `prepare` (a new video story, runs at once): probe, refuse when too long or without
//!   video, then either remux (an upload already in the live format) or transcode to
//!   H.264/AAC MP4, plus a JPEG poster.
//! - `archive` (an expired story, not urgent): heavy compression, only claimed within
//!   `ARCHIVE_HOURS` (local time, default `2-6`, empty = any time).
//!
//! Env: `SERVER_URL` (default `http://server:8081`), `WORKER_TOKEN` (required),
//! `FFMPEG_THREADS` (default `1`), `ARCHIVE_HOURS`.

use proto::worker::{EncodeProfile, JobKind, WorkerFailure, WorkerJob, WorkerResult};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};

/// Largest input the server hands out (a story video upload).
const INPUT_BYTES: u64 = 61 * 1024 * 1024;
/// Largest main output the server accepts.
const MAIN_BYTES: u64 = 40 * 1024 * 1024;
/// ffmpeg/ffprobe runs are killed after this.
const TOOL_TIMEOUT: Duration = Duration::from_secs(300);

struct Config {
    server: String,
    token: String,
    threads: u32,
    /// Local hours `[start, end)` when archive jobs may run; none = any time.
    archive_hours: Option<(u32, u32)>,
    /// Run ffmpeg under `nice -n 19`.
    nice: bool,
}

/// Why a job did not complete.
#[derive(Debug, PartialEq)]
enum Failure {
    /// Transient (I/O, HTTP, timeout): worth another attempt.
    Retry(String),
    /// The input is refused (too long, no video, undecodable).
    Refuse(String),
}

type Outcome<T> = Result<T, Failure>;

fn retry(e: impl std::fmt::Display) -> Failure {
    Failure::Retry(e.to_string())
}

/// "2-6" → (2, 6): from 02:00 to 06:00. "22-5" wraps past midnight.
fn parse_hours(text: &str) -> Option<(u32, u32)> {
    let (a, b) = text.trim().split_once('-')?;
    let (a, b): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a <= 24 && b <= 24 && a % 24 != b % 24).then_some((a % 24, b % 24))
}

/// Whether archive jobs may run at this local hour.
fn archive_allowed(hours: Option<(u32, u32)>, hour: u32) -> bool {
    match hours {
        None => true,
        Some((a, b)) if a < b => a <= hour && hour < b,
        Some((a, b)) => hour >= a || hour < b,
    }
}

fn local_hour() -> u32 {
    use chrono::Timelike;
    chrono::Local::now().hour()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string());
    let token = var("WORKER_TOKEN").unwrap_or_default();
    if token.is_empty() {
        eprintln!("story-worker: WORKER_TOKEN is required");
        std::process::exit(1);
    }
    let archive_hours = match var("ARCHIVE_HOURS") {
        None => parse_hours("2-6"),
        Some(v) if v.is_empty() => None,
        Some(v) => match parse_hours(&v) {
            Some(h) => Some(h),
            None => {
                eprintln!("story-worker: ARCHIVE_HOURS must look like 2-6 (or be empty)");
                std::process::exit(1);
            }
        },
    };
    let nice = Command::new("nice")
        .args(["-n", "19", "true"])
        .stdin(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    let config = Config {
        server: var("SERVER_URL").filter(|v| !v.is_empty()).unwrap_or_else(|| "http://server:8081".into())
            .trim_end_matches('/')
            .to_string(),
        token,
        threads: var("FFMPEG_THREADS").and_then(|v| v.parse().ok()).filter(|&n| n > 0).unwrap_or(1),
        archive_hours,
        nice,
    };
    let client = reqwest::Client::builder()
        .user_agent("GoofyChat-story-worker")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .build()
        .expect("http client");
    tracing::info!(
        "story worker: {} ({} ffmpeg threads, nice {}, archive hours {:?})",
        config.server,
        config.threads,
        config.nice,
        config.archive_hours
    );
    // One job at a time, in the order the server hands them out.
    loop {
        let archive = archive_allowed(config.archive_hours, local_hour());
        match claim(&config, &client, archive).await {
            Ok(None) => tokio::time::sleep(Duration::from_secs(5)).await,
            Ok(Some(job)) => run_job(&config, &client, job).await,
            Err(e) => {
                tracing::warn!("claim: {e:?}");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}

fn url(config: &Config, path: &str) -> String {
    format!("{}/internal/jobs/{path}", config.server)
}

fn authed(config: &Config, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request.bearer_auth(&config.token)
}

async fn check(response: reqwest::Response) -> Outcome<reqwest::Response> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(Failure::Retry(format!("HTTP {}", response.status())))
    }
}

async fn claim(config: &Config, client: &reqwest::Client, archive: bool) -> Outcome<Option<WorkerJob>> {
    let request = client.post(url(config, &format!("claim?archive={archive}")));
    let response = check(authed(config, request).send().await.map_err(retry)?).await?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    let bytes = response.bytes().await.map_err(retry)?;
    serde_json::from_slice(&bytes).map(Some).map_err(retry)
}

async fn post_json(config: &Config, client: &reqwest::Client, path: &str, body: &impl serde::Serialize) -> Outcome<()> {
    let body = serde_json::to_vec(body).map_err(retry)?;
    let request = client.post(url(config, path)).header("content-type", "application/json").body(body);
    check(authed(config, request).send().await.map_err(retry)?).await?;
    Ok(())
}

async fn run_job(config: &Config, client: &reqwest::Client, job: WorkerJob) {
    let dir = std::env::temp_dir().join(format!("job-{}", job.id));
    tracing::info!("job {}: {:?} ({})", job.id, job.kind, job.input_mime);
    let outcome = process(config, client, &job, &dir).await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let (error, retry) = match outcome {
        Ok(()) => {
            tracing::info!("job {} done", job.id);
            return;
        }
        Err(Failure::Retry(e)) => (e, true),
        Err(Failure::Refuse(e)) => (e, false),
    };
    tracing::warn!("job {} failed (retry {retry}): {error}", job.id);
    let failure = WorkerFailure { error, retry };
    if let Err(e) = post_json(config, client, &format!("{}/fail", job.id), &failure).await {
        tracing::warn!("job {}: could not report the failure: {e:?}", job.id);
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "bin",
    }
}

async fn download(config: &Config, client: &reqwest::Client, job: i64, to: &Path) -> Outcome<u64> {
    let request = client.get(url(config, &format!("{job}/input")));
    let mut response = check(authed(config, request).send().await.map_err(retry)?).await?;
    let mut file = tokio::fs::File::create(to).await.map_err(retry)?;
    let mut size = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(retry)? {
        size += chunk.len() as u64;
        if size > INPUT_BYTES {
            return Err(Failure::Refuse("input too large".into()));
        }
        file.write_all(&chunk).await.map_err(retry)?;
    }
    file.flush().await.map_err(retry)?;
    Ok(size)
}

async fn upload(config: &Config, client: &reqwest::Client, job: i64, slot: &str, mime: &str, file: &Path) -> Outcome<()> {
    let bytes = tokio::fs::read(file).await.map_err(retry)?;
    let request = client
        .put(url(config, &format!("{job}/output/{slot}")))
        .header("content-type", mime)
        .body(bytes);
    check(authed(config, request).send().await.map_err(retry)?).await?;
    Ok(())
}

/// Runs a tool; a non-zero exit means the input is refused.
async fn tool(config: &Config, program: &str, args: &[String], low_priority: bool) -> Outcome<Vec<u8>> {
    let mut command = if low_priority && config.nice {
        let mut c = Command::new("nice");
        c.args(["-n", "19", program]);
        c
    } else {
        Command::new(program)
    };
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().map_err(|e| Failure::Retry(format!("{program}: {e}")))?;
    // Dropping the child on timeout kills it.
    match tokio::time::timeout(TOOL_TIMEOUT, child.wait_with_output()).await {
        Err(_) => Err(Failure::Retry(format!("{program}: timeout"))),
        Ok(Err(e)) => Err(Failure::Retry(format!("{program}: {e}"))),
        Ok(Ok(out)) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
            Err(Failure::Refuse(format!("{program} failed: {tail}")))
        }
        Ok(Ok(out)) => Ok(out.stdout),
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Keeps the aspect ratio, never upscales, and keeps both sides even (x264 and yuv420p
/// need it).
fn scale(side: u32) -> String {
    format!(
        "scale='if(gte(iw,ih),trunc(min({side},iw)/2)*2,-2)':'if(gte(iw,ih),-2,trunc(min({side},ih)/2)*2)'"
    )
}

#[derive(Debug, Default, PartialEq)]
struct VideoStream {
    codec: String,
    pix_fmt: String,
    /// Displayed size (rotation applied).
    width: i64,
    height: i64,
    fps: f64,
}

#[derive(Debug, Default, PartialEq)]
struct Probe {
    duration_ms: i64,
    /// Whole-file bit rate, when known.
    bit_rate_kbps: Option<i64>,
    video: Option<VideoStream>,
    /// Codec of the first audio stream.
    audio: Option<String>,
}

fn rate(text: &str) -> f64 {
    match text.split_once('/') {
        Some((n, d)) => match (n.parse::<f64>(), d.parse::<f64>()) {
            (Ok(n), Ok(d)) if d > 0.0 => n / d,
            _ => 0.0,
        },
        None => text.parse().unwrap_or(0.0),
    }
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Reads `ffprobe -print_format json -show_format -show_streams`.
fn parse_probe(json: &Value) -> Probe {
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1);
    let video = video.map(|s| {
        let rotation = number(&s["tags"]["rotate"])
            .or_else(|| s["side_data_list"].as_array().and_then(|l| l.iter().find_map(|d| number(&d["rotation"]))))
            .unwrap_or(0.0) as i64;
        let (w, h) = (s["width"].as_i64().unwrap_or(0), s["height"].as_i64().unwrap_or(0));
        let (width, height) = if rotation.rem_euclid(180) == 90 { (h, w) } else { (w, h) };
        let mut fps = rate(s["avg_frame_rate"].as_str().unwrap_or(""));
        if fps <= 0.0 {
            fps = rate(s["r_frame_rate"].as_str().unwrap_or(""));
        }
        VideoStream {
            codec: s["codec_name"].as_str().unwrap_or("").into(),
            pix_fmt: s["pix_fmt"].as_str().unwrap_or("").into(),
            width,
            height,
            fps,
        }
    });
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").map(|s| s["codec_name"].as_str().unwrap_or("").to_string());
    let seconds = number(&json["format"]["duration"])
        .or_else(|| streams.iter().find_map(|s| number(&s["duration"])))
        .unwrap_or(0.0);
    let bit_rate_kbps = number(&json["format"]["bit_rate"]).map(|b| (b / 1000.0) as i64);
    Probe { duration_ms: (seconds * 1000.0).round() as i64, bit_rate_kbps, video, audio }
}

/// An upload already in the live format (H.264 yuv420p + AAC or no audio, at most
/// 1280x720 either way, at most the profile's frame rate, no heavier than an encode
/// would be) is only remuxed.
fn fits_live(probe: &Probe, profile: &EncodeProfile, size: u64) -> bool {
    let Some(v) = &probe.video else { return false };
    let long = v.width.max(v.height);
    let short = v.width.min(v.height);
    let max_short = (profile.max_side as i64) * 9 / 16;
    let seconds = (probe.duration_ms as f64 / 1000.0).max(0.1);
    let kbps = probe.bit_rate_kbps.unwrap_or((size as f64 * 8.0 / 1000.0 / seconds) as i64);
    let budget = (profile.max_kbps + profile.audio_kbps) as i64 * 5 / 4;
    v.codec == "h264"
        && matches!(v.pix_fmt.as_str(), "yuv420p" | "yuvj420p")
        && probe.audio.as_deref().is_none_or(|a| a == "aac")
        && long > 0
        && long <= profile.max_side as i64
        && short <= max_short
        && v.fps > 0.0
        && v.fps <= profile.fps as f64 + 0.5
        && size <= MAIN_BYTES
        && kbps <= budget
}

async fn probe(config: &Config, file: &Path) -> Outcome<Probe> {
    let out = tool(
        config,
        "ffprobe",
        &[args(&["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]), vec![path(file)]].concat(),
        false,
    )
    .await?;
    let json: Value = serde_json::from_slice(&out).map_err(|_| Failure::Refuse("unreadable probe".into()))?;
    Ok(parse_probe(&json))
}

fn encode_args(input: &Path, out: &Path, p: &EncodeProfile, threads: u32) -> Vec<String> {
    let mut a = args(&["-nostdin", "-y", "-i"]);
    a.push(path(input));
    a.extend(args(&["-t", "30", "-map", "0:v:0", "-map", "0:a:0?", "-vf"]));
    a.push(format!("{},fps={}", scale(p.max_side), p.fps));
    a.extend(args(&["-c:v", "libx264", "-preset", "veryfast", "-crf"]));
    a.push(p.crf.to_string());
    a.push("-maxrate".into());
    a.push(format!("{}k", p.max_kbps));
    a.push("-bufsize".into());
    a.push(format!("{}k", p.max_kbps * 2));
    a.extend(args(&["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a"]));
    a.push(format!("{}k", p.audio_kbps));
    a.push("-ac".into());
    a.push(p.audio_channels.max(1).to_string());
    a.extend(args(&["-movflags", "+faststart", "-map_metadata", "-1", "-threads"]));
    a.push(threads.to_string());
    a.push(path(out));
    a
}

fn remux_args(input: &Path, out: &Path) -> Vec<String> {
    let mut a = args(&["-nostdin", "-y", "-i"]);
    a.push(path(input));
    a.extend(args(&["-t", "30", "-map", "0:v:0", "-map", "0:a:0?", "-c", "copy", "-movflags", "+faststart", "-map_metadata", "-1"]));
    a.push(path(out));
    a
}

fn poster_args(video: &Path, out: &Path, p: &EncodeProfile, threads: u32) -> Vec<String> {
    let mut a = args(&["-nostdin", "-y", "-i"]);
    a.push(path(video));
    a.extend(args(&["-frames:v", "1", "-vf"]));
    a.push(scale(p.poster_side));
    a.push("-q:v".into());
    a.push(p.jpeg_q.to_string());
    a.push("-threads".into());
    a.push(threads.to_string());
    a.push(path(out));
    a
}

fn photo_args(input: &Path, out: &Path, p: &EncodeProfile, threads: u32) -> Vec<String> {
    let mut a = args(&["-nostdin", "-y", "-i"]);
    a.push(path(input));
    a.extend(args(&["-frames:v", "1", "-vf"]));
    a.push(scale(p.max_side));
    a.push("-q:v".into());
    a.push(p.jpeg_q.to_string());
    a.extend(args(&["-map_metadata", "-1", "-threads"]));
    a.push(threads.to_string());
    a.push(path(out));
    a
}

async fn process(config: &Config, client: &reqwest::Client, job: &WorkerJob, dir: &PathBuf) -> Outcome<()> {
    tokio::fs::create_dir_all(dir).await.map_err(retry)?;
    let input = dir.join(format!("in.{}", extension(&job.input_mime)));
    let size = download(config, client, job.id, &input).await?;
    let p = &job.profile;
    let result = if job.is_video {
        let found = probe(config, &input).await?;
        if found.video.is_none() {
            return Err(Failure::Refuse("no video stream".into()));
        }
        if p.max_duration_ms > 0 && found.duration_ms > p.max_duration_ms + 500 {
            return Err(Failure::Refuse(format!("too long: {} ms", found.duration_ms)));
        }
        let out = dir.join("out.mp4");
        if job.kind == JobKind::Prepare && fits_live(&found, p, size) {
            tracing::info!("job {}: already in the live format, remuxing", job.id);
            tool(config, "ffmpeg", &remux_args(&input, &out), true).await?;
        } else {
            tool(config, "ffmpeg", &encode_args(&input, &out, p, config.threads), true).await?;
        }
        let poster = dir.join("poster.jpg");
        tool(config, "ffmpeg", &poster_args(&out, &poster, p, config.threads), true).await?;
        let made = probe(config, &out).await?;
        let v = made.video.ok_or_else(|| Failure::Refuse("no video in the output".into()))?;
        upload(config, client, job.id, "main", "video/mp4", &out).await?;
        upload(config, client, job.id, "poster", "image/jpeg", &poster).await?;
        WorkerResult { width: v.width, height: v.height, duration_ms: made.duration_ms }
    } else {
        let out = dir.join("out.jpg");
        tool(config, "ffmpeg", &photo_args(&input, &out, p, config.threads), true).await?;
        let made = probe(config, &out).await?;
        let v = made.video.ok_or_else(|| Failure::Refuse("no image in the output".into()))?;
        upload(config, client, job.id, "main", "image/jpeg", &out).await?;
        WorkerResult { width: v.width, height: v.height, duration_ms: 0 }
    };
    post_json(config, client, &format!("{}/done", job.id), &result).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn archive_hours() {
        assert_eq!(parse_hours("2-6"), Some((2, 6)));
        assert_eq!(parse_hours(" 22 - 5 "), Some((22, 5)));
        assert_eq!(parse_hours("0-24"), None, "same hour both ends");
        assert_eq!(parse_hours("6"), None);
        assert_eq!(parse_hours("a-b"), None);
        let night = parse_hours("2-6");
        let allowed: Vec<u32> = (0..24).filter(|&h| archive_allowed(night, h)).collect();
        assert_eq!(allowed, [2, 3, 4, 5]);
        let wrap = parse_hours("22-1");
        let allowed: Vec<u32> = (0..24).filter(|&h| archive_allowed(wrap, h)).collect();
        assert_eq!(allowed, [0, 22, 23]);
        assert!((0..24).all(|h| archive_allowed(None, h)), "empty means any time");
    }

    fn prepare() -> EncodeProfile {
        EncodeProfile {
            max_side: 1280,
            fps: 30,
            crf: 26,
            max_kbps: 2500,
            audio_kbps: 128,
            audio_channels: 2,
            jpeg_q: 4,
            poster_side: 720,
            max_duration_ms: 30_000,
        }
    }

    fn phone_probe(codec: &str, width: i64, height: i64, fps: &str, rotate: Option<i64>) -> Value {
        let mut video = json!({"codec_type": "video", "codec_name": codec, "pix_fmt": "yuv420p",
            "width": width, "height": height, "avg_frame_rate": fps, "r_frame_rate": fps});
        if let Some(r) = rotate {
            video["side_data_list"] = json!([{"side_data_type": "Display Matrix", "rotation": r}]);
        }
        json!({"streams": [video, {"codec_type": "audio", "codec_name": "aac"}],
               "format": {"duration": "12.480000", "bit_rate": "2000000"}})
    }

    #[test]
    fn probe_and_remux_decision() {
        let p = parse_probe(&phone_probe("h264", 1280, 720, "30000/1001", Some(-90)));
        let v = p.video.as_ref().unwrap();
        assert_eq!((v.width, v.height), (720, 1280), "rotation applied");
        assert!((v.fps - 29.97).abs() < 0.01);
        assert_eq!((p.duration_ms, p.bit_rate_kbps, p.audio.as_deref()), (12_480, Some(2000), Some("aac")));
        assert!(fits_live(&p, &prepare(), 3_000_000));

        let big = parse_probe(&phone_probe("h264", 1920, 1080, "30/1", None));
        assert!(!fits_live(&big, &prepare(), 3_000_000), "1080p is re-encoded");
        let hevc = parse_probe(&phone_probe("hevc", 1280, 720, "30/1", None));
        assert!(!fits_live(&hevc, &prepare(), 3_000_000));
        let fast = parse_probe(&phone_probe("h264", 1280, 720, "60/1", None));
        assert!(!fits_live(&fast, &prepare(), 3_000_000), "60 fps is re-encoded");
        let mut heavy = parse_probe(&phone_probe("h264", 1280, 720, "30/1", None));
        heavy.bit_rate_kbps = Some(12_000);
        assert!(!fits_live(&heavy, &prepare(), 3_000_000), "too heavy for the live budget");
        let mut opus = parse_probe(&phone_probe("h264", 1280, 720, "30/1", None));
        opus.audio = Some("opus".into());
        assert!(!fits_live(&opus, &prepare(), 3_000_000));
        let mut silent = parse_probe(&phone_probe("h264", 1280, 720, "30/1", None));
        silent.audio = None;
        assert!(fits_live(&silent, &prepare(), 3_000_000), "no audio is fine");
        assert_eq!(parse_probe(&json!({"streams": [], "format": {}})).video, None);
    }

    #[test]
    fn commands() {
        assert_eq!(
            scale(1280),
            "scale='if(gte(iw,ih),trunc(min(1280,iw)/2)*2,-2)':'if(gte(iw,ih),-2,trunc(min(1280,ih)/2)*2)'"
        );
        let a = encode_args(Path::new("/tmp/in.mov"), Path::new("/tmp/out.mp4"), &prepare(), 1);
        let line = a.join(" ");
        assert!(line.contains("-c:v libx264 -preset veryfast -crf 26 -maxrate 2500k -bufsize 5000k"), "{line}");
        assert!(line.contains(",fps=30") && line.contains("-ac 2") && line.ends_with("-threads 1 /tmp/out.mp4"));
        let r = remux_args(Path::new("in.mp4"), Path::new("out.mp4")).join(" ");
        assert!(r.contains("-c copy -movflags +faststart"));
    }
}
