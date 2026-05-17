use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tauri::{AppHandle, Emitter};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub url: String,
    pub opts: serde_json::Value,
}

pub struct CurrentJob {
    pub prefix: String,
    pub queue: Arc<Mutex<Vec<DownloadItem>>>,
    pub cancelled: Arc<AtomicBool>,
    pub child_pid: Arc<Mutex<Option<u32>>>,
}

fn build_args(
    platform: &str,
    url: &str,
    dir: &str,
    opts: &serde_json::Value,
    ffmpeg_bin: &str,
) -> Vec<String> {
    let template = opts["template"]
        .as_str()
        .unwrap_or("%(title)s.%(ext)s");
    let tmpl = format!("{}/{}", dir.trim_end_matches('/'), template);

    let format = opts["format"].as_str().unwrap_or("mp4").to_lowercase();
    let quality = opts["quality"].as_str().unwrap_or("bestvideo+bestaudio/best");
    let otype = opts["type"].as_str().unwrap_or("video");
    let cookies = opts["cookies"].as_str();

    // Base flags applied to every invocation.
    // --concurrent-fragments speeds up YouTube HLS/DASH by downloading 4 segments in parallel.
    let mut base = vec![
        "--ffmpeg-location".to_string(), ffmpeg_bin.to_string(),
        "--no-warnings".to_string(),
        "--concurrent-fragments".to_string(), "4".to_string(),
    ];

    // Normalise a video-container format string to a valid audio codec for --audio-format.
    let to_audio_fmt = |f: &str| -> String {
        if ["mp4", "mkv", "webm", "mov"].contains(&f) { "mp3".to_string() } else { f.to_string() }
    };
    // Validate the format for --convert-thumbnails — fall back to jpg if unsupported.
    let to_thumb_fmt = |f: &str| -> String {
        if ["jpg", "jpeg", "png", "webp"].contains(&f) { f.to_string() } else { "jpg".to_string() }
    };

    match platform {
        "yt" => {
            if otype == "thumbnail" {
                base.extend([
                    "--write-thumbnail".to_string(),
                    "--skip-download".to_string(),
                    "--convert-thumbnails".to_string(),
                    to_thumb_fmt(&format),
                    "-o".to_string(), tmpl,
                    "--no-playlist".to_string(),
                    url.to_string(),
                ]);
                base
            } else if otype == "audio" || quality == "bestaudio/best" {
                base.extend([
                    "--extract-audio".to_string(),
                    "--audio-format".to_string(), to_audio_fmt(&format),
                    "-o".to_string(), tmpl,
                    "--no-playlist".to_string(),
                    url.to_string(),
                ]);
                base
            } else {
                base.extend([
                    "-f".to_string(), quality.to_string(),
                    "--merge-output-format".to_string(), format.clone(),
                    "-o".to_string(), tmpl,
                    "--no-playlist".to_string(),
                    url.to_string(),
                ]);
                base
            }
        }
        "ig" => {
            if otype == "thumbnail" {
                base.extend([
                    "--write-thumbnail".to_string(),
                    "--skip-download".to_string(),
                    "--convert-thumbnails".to_string(),
                    to_thumb_fmt(&format),
                    "-o".to_string(), tmpl,
                    "--no-playlist".to_string(),
                ]);
                if let Some(c) = cookies {
                    base.push("--cookies-from-browser".to_string());
                    base.push(c.to_string());
                }
                base.push(url.to_string());
                base
            } else if otype == "audio" {
                base.extend([
                    "--extract-audio".to_string(),
                    "--audio-format".to_string(), to_audio_fmt(&format),
                    "-o".to_string(), tmpl,
                    "--no-playlist".to_string(),
                ]);
                if let Some(c) = cookies {
                    base.push("--cookies-from-browser".to_string());
                    base.push(c.to_string());
                }
                base.push(url.to_string());
                base
            } else if otype == "image" {
                base.extend(["-o".to_string(), tmpl, "--no-playlist".to_string()]);
                if let Some(c) = cookies {
                    base.push("--cookies-from-browser".to_string());
                    base.push(c.to_string());
                }
                base.push(url.to_string());
                base
            } else {
                // Instagram serves combined streams. --recode-video is a no-op when the
                // source container already matches the target (the common mp4→mp4 case)
                // but transparently transcodes if a fallback stream has incompatible codecs
                // (e.g. webm with vp9/opus → mp4). --remux-video would fail there.
                base.extend([
                    "-f".to_string(), quality.to_string(),
                    "--recode-video".to_string(), format.clone(),
                    "-o".to_string(), tmpl,
                    "--no-playlist".to_string(),
                ]);
                if let Some(c) = cookies {
                    base.push("--cookies-from-browser".to_string());
                    base.push(c.to_string());
                }
                base.push(url.to_string());
                base
            }
        }
        _ => vec![url.to_string()],
    }
}

struct ProcResult {
    code: i32,
    skipped_path: Option<String>,
    // Last few stderr lines — surfaces real error context when yt-dlp exits non-zero.
    stderr_tail: String,
}

async fn run_proc(
    ytdlp_bin: &str,
    args: &[String],
    dir: &str,
    prefix: &str,
    idx: usize,
    total: usize,
    cancelled: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    app: &AppHandle,
) -> Result<ProcResult, String> {
    let mut cmd = Command::new(ytdlp_bin);
    cmd.args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env(
            "PATH",
            "/Library/Frameworks/Python.framework/Versions/3.12/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        );

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    // Store PID
    if let Some(pid) = child.id() {
        *child_pid.lock().await = Some(pid);
    }

    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let prefix_owned = prefix.to_string();
    let app1 = app.clone();
    let app2 = app.clone();

    let mut skipped_path: Option<String> = None;
    // Ring buffer of the last 5 stderr lines so we can surface real error context if yt-dlp dies.
    let mut stderr_recent: std::collections::VecDeque<String> = std::collections::VecDeque::with_capacity(5);

    loop {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        tokio::select! {
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        if text.trim().is_empty() { continue; }
                        let _ = app1.emit("download:log", serde_json::json!({
                            "prefix": prefix_owned,
                            "text": text.clone(),
                        }));

                        // Check for "already been downloaded"
                        let already_marker = "has already been downloaded";
                        if let Some(pos) = text.find("[download] ") {
                            if let Some(end) = text.find(already_marker) {
                                let path_part = text[pos + "[download] ".len()..end].trim();
                                skipped_path = Some(path_part.to_string());
                            }
                        }

                        // Parse progress percentage
                        if let Some(pct_val) = parse_percent(&text) {
                            let overall = ((idx as f64 / total as f64)
                                + (pct_val / 100.0 / total as f64))
                                * 100.0;
                            let _ = app1.emit("download:progress", serde_json::json!({
                                "prefix": prefix_owned,
                                "pct": overall.round() as u32,
                            }));
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        if text.trim().is_empty() { continue; }
                        if stderr_recent.len() == 5 { stderr_recent.pop_front(); }
                        stderr_recent.push_back(text.clone());
                        let _ = app2.emit("download:log", serde_json::json!({
                            "prefix": prefix_owned,
                            "text": text,
                            "type": "warn",
                        }));
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            }
        }
    }

    // Drain remaining stderr
    while let Ok(Some(text)) = stderr_reader.next_line().await {
        if !text.trim().is_empty() {
            if stderr_recent.len() == 5 { stderr_recent.pop_front(); }
            stderr_recent.push_back(text.clone());
            let _ = app.emit("download:log", serde_json::json!({
                "prefix": prefix,
                "text": text,
                "type": "warn",
            }));
        }
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    *child_pid.lock().await = None;

    let code = status.code().unwrap_or(-1);
    // Prefer the line containing ERROR: if present, else the last line.
    let stderr_tail = stderr_recent.iter()
        .rev()
        .find(|l| l.contains("ERROR:"))
        .cloned()
        .or_else(|| stderr_recent.back().cloned())
        .unwrap_or_default();
    Ok(ProcResult { code, skipped_path, stderr_tail })
}

fn parse_percent(text: &str) -> Option<f64> {
    // Find "XX.X%" pattern
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i > 0 {
            // Walk back to find start of number
            let mut j = i;
            while j > 0 && (bytes[j - 1].is_ascii_digit() || bytes[j - 1] == b'.') {
                j -= 1;
            }
            if j < i {
                if let Ok(v) = text[j..i].parse::<f64>() {
                    return Some(v);
                }
            }
        }
        i += 1;
    }
    None
}

pub async fn run_download_job(
    app: AppHandle,
    ytdlp_bin: String,
    ffmpeg_bin: String,
    prefix: String,
    items: Vec<DownloadItem>,
    dir: String,
    job_arc: Arc<CurrentJob>,
) {
    let send_log = |text: &str, log_type: Option<&str>| {
        let mut payload = serde_json::json!({ "prefix": prefix, "text": text });
        if let Some(t) = log_type {
            payload["type"] = serde_json::json!(t);
        }
        let _ = app.emit("download:log", payload);
    };

    let send_prog = |pct: u32| {
        let _ = app.emit("download:progress", serde_json::json!({ "prefix": prefix, "pct": pct }));
    };

    let send_item = |idx: usize, status: &str, label: &str| {
        let _ = app.emit(
            "download:item-status",
            serde_json::json!({ "prefix": prefix, "index": idx, "status": status, "label": label }),
        );
    };

    let send_done = |ok: bool| {
        let _ = app.emit("download:complete", serde_json::json!({ "prefix": prefix, "success": ok }));
    };

    // Create output directory
    if std::fs::create_dir_all(&dir).is_err() {
        send_log("Cannot create output directory", Some("error"));
        send_done(false);
        return;
    }

    // Initialize queue with initial items
    {
        let mut q = job_arc.queue.lock().await;
        *q = items;
    }

    let mut idx: usize = 0;

    loop {
        if job_arc.cancelled.load(Ordering::Relaxed) {
            send_log("Cancelled.", Some("warn"));
            send_done(false);
            return;
        }

        let item = {
            let q = job_arc.queue.lock().await;
            q.get(idx).cloned()
        };

        let item = match item {
            Some(i) => i,
            None => {
                send_prog(100);
                send_log("All downloads complete.", Some("success"));
                send_done(true);
                return;
            }
        };

        let total = {
            let q = job_arc.queue.lock().await;
            q.len()
        };

        let url = item.url.trim().to_string();
        if url.is_empty() {
            idx += 1;
            continue;
        }

        let opts = &item.opts;

        {
            send_item(idx, "active", "downloading\u{2026}");
            send_log(
                &format!("\u{2193} [{}/{}] {}", idx + 1, total, url),
                Some("info"),
            );

            let mut args = build_args(&prefix, &url, &dir, opts, &ffmpeg_bin);
            args.push("--no-overwrites".to_string());

            match run_proc(
                &ytdlp_bin,
                &args,
                &dir,
                &prefix,
                idx,
                total,
                job_arc.cancelled.clone(),
                job_arc.child_pid.clone(),
                &app,
            )
            .await
            {
                Ok(result) => {
                    if job_arc.cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    if result.code == 0 {
                        if let Some(skipped) = result.skipped_path {
                            send_item(idx, "done", "exists \u{2713}");
                            send_log(
                                &format!("Already downloaded: {}", skipped),
                                Some("info"),
                            );
                        } else {
                            send_item(idx, "done", "done \u{2713}");
                            send_log(&format!("Saved to {}", dir), Some("success"));
                        }
                    } else {
                        send_item(idx, "error", "error \u{2717}");
                        let tail = if result.stderr_tail.is_empty() {
                            format!("Failed (exit {})", result.code)
                        } else {
                            format!("Failed (exit {}): {}", result.code, result.stderr_tail)
                        };
                        send_log(&tail, Some("error"));
                    }
                }
                Err(e) => {
                    if job_arc.cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    send_item(idx, "error", "error \u{2717}");
                    send_log(&format!("Process error: {}", e), Some("error"));
                }
            }
        }

        idx += 1;
    }
}

pub fn kill_child(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    }
}
