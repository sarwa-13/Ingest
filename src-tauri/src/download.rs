use std::path::{Path, PathBuf};
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

fn get_available_path(file_path: &Path) -> PathBuf {
    if !file_path.exists() {
        return file_path.to_owned();
    }
    let dir = file_path.parent().unwrap_or(Path::new("."));
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");

    for n in 1..=999 {
        let name = if ext.is_empty() {
            format!("{} ({})", stem, n)
        } else {
            format!("{} ({}).{}", stem, n, ext)
        };
        let candidate = dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }
    file_path.to_owned()
}

fn build_args(
    platform: &str,
    url: &str,
    dir: &str,
    opts: &serde_json::Value,
    ffmpeg_bin: &str,
    output_override: Option<&str>,
) -> Vec<String> {
    let template = opts["template"]
        .as_str()
        .unwrap_or("%(title)s.%(ext)s");
    let tmpl = output_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}/{}", dir.trim_end_matches('/'), template));

    let format = opts["format"].as_str().unwrap_or("mp4").to_lowercase();
    let quality = opts["quality"].as_str().unwrap_or("bestvideo+bestaudio/best");
    let otype = opts["type"].as_str().unwrap_or("video");
    let cookies = opts["cookies"].as_str();

    let ff_loc = vec!["--ffmpeg-location".to_string(), ffmpeg_bin.to_string()];

    match platform {
        "yt" => {
            if otype == "thumbnail" {
                let mut args = ff_loc;
                args.extend([
                    "--write-thumbnail".to_string(),
                    "--skip-download".to_string(),
                    "--convert-thumbnails".to_string(),
                    format.clone(),
                    "-o".to_string(),
                    tmpl,
                    "--no-playlist".to_string(),
                    url.to_string(),
                ]);
                args
            } else if quality == "bestaudio/best" {
                let audio_fmt = if ["mp4", "mkv", "webm"].contains(&format.as_str()) {
                    "mp3".to_string()
                } else {
                    format.clone()
                };
                let mut args = ff_loc;
                args.extend([
                    "--extract-audio".to_string(),
                    "--audio-format".to_string(),
                    audio_fmt,
                    "-o".to_string(),
                    tmpl,
                    "--no-playlist".to_string(),
                    url.to_string(),
                ]);
                args
            } else {
                let mut args = ff_loc;
                args.extend([
                    "-f".to_string(),
                    quality.to_string(),
                    "--merge-output-format".to_string(),
                    format.clone(),
                    "-o".to_string(),
                    tmpl,
                    "--no-playlist".to_string(),
                    url.to_string(),
                ]);
                args
            }
        }
        "ig" => {
            if otype == "thumbnail" {
                let mut args = ff_loc;
                args.extend([
                    "--write-thumbnail".to_string(),
                    "--skip-download".to_string(),
                    "--convert-thumbnails".to_string(),
                    format.clone(),
                    "-o".to_string(),
                    tmpl,
                    "--no-playlist".to_string(),
                ]);
                if let Some(c) = cookies {
                    args.push("--cookies-from-browser".to_string());
                    args.push(c.to_string());
                }
                args.push(url.to_string());
                args
            } else if otype == "audio" {
                let mut args = ff_loc;
                args.extend([
                    "--extract-audio".to_string(),
                    "--audio-format".to_string(),
                    format.clone(),
                    "-o".to_string(),
                    tmpl,
                    "--no-playlist".to_string(),
                ]);
                if let Some(c) = cookies {
                    args.push("--cookies-from-browser".to_string());
                    args.push(c.to_string());
                }
                args.push(url.to_string());
                args
            } else if otype == "image" {
                let mut args = ff_loc;
                args.extend([
                    "-o".to_string(),
                    tmpl,
                    "--no-playlist".to_string(),
                ]);
                if let Some(c) = cookies {
                    args.push("--cookies-from-browser".to_string());
                    args.push(c.to_string());
                }
                args.push(url.to_string());
                args
            } else {
                let mut args = ff_loc;
                args.extend([
                    "-f".to_string(),
                    quality.to_string(),
                    "--merge-output-format".to_string(),
                    format.clone(),
                    "-o".to_string(),
                    tmpl,
                ]);
                if let Some(c) = cookies {
                    args.push("--cookies-from-browser".to_string());
                    args.push(c.to_string());
                }
                args.push(url.to_string());
                args
            }
        }
        _ => vec![url.to_string()],
    }
}

async fn resolve_spotify(url: &str) -> Result<(String, String), String> {
    let clean = url.split('?').next().unwrap_or(url);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(clean)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let html = resp.text().await.map_err(|e| e.to_string())?;

    // Parse og:description
    let needle = "property=\"og:description\" content=\"";
    if let Some(start) = html.find(needle) {
        let rest = &html[start + needle.len()..];
        if let Some(end) = rest.find('"') {
            let content = &rest[..end];
            let parts: Vec<&str> = content.split(" \u{00B7} ").collect();
            if parts.len() >= 2 {
                let artists = parts[0].trim().to_string();
                let title = parts[1].trim().to_string();
                let search_query = format!("{} {}", title, artists);
                return Ok((title, search_query));
            } else {
                return Ok((content.to_string(), content.to_string()));
            }
        }
    }

    Err("Track info not found".to_string())
}

struct ProcResult {
    code: i32,
    skipped_path: Option<String>,
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
    Ok(ProcResult { code, skipped_path })
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

        if prefix == "sp" {
            let mut search_query = url.clone();
            if url.contains("open.spotify.com/track") {
                send_item(idx, "active", "resolving\u{2026}");
                send_log("\u{2192} Resolving Spotify URL\u{2026}", Some("info"));
                match resolve_spotify(&url).await {
                    Ok((title, sq)) => {
                        search_query = sq;
                        send_log(&format!("\u{266a} Found: {}", title), Some("success"));
                    }
                    Err(e) => {
                        send_log(&format!("Could not resolve URL: {}", e), Some("warn"));
                    }
                }
            }

            send_item(idx, "active", "downloading\u{2026}");
            send_log(
                &format!(
                    "\u{2193} [{}/{}] Searching: {}",
                    idx + 1,
                    total,
                    search_query
                ),
                Some("info"),
            );

            let format = opts["format"].as_str().unwrap_or("mp3").to_lowercase();
            let template = opts["template"].as_str().unwrap_or("%(title)s.%(ext)s");
            let tmpl = format!("{}/{}", dir.trim_end_matches('/'), template);

            let sp_args = vec![
                "--ffmpeg-location".to_string(),
                ffmpeg_bin.clone(),
                format!("ytsearch1:{}", search_query),
                "--extract-audio".to_string(),
                "--audio-format".to_string(),
                format.clone(),
                "--audio-quality".to_string(),
                "0".to_string(),
                "-o".to_string(),
                tmpl.clone(),
                "--no-playlist".to_string(),
                "--no-overwrites".to_string(),
            ];

            match run_proc(
                &ytdlp_bin,
                &sp_args,
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

                    if let Some(skipped) = result.skipped_path {
                        let numbered = get_available_path(Path::new(&skipped));
                        send_log(
                            &format!(
                                "File exists \u{2014} saving as: {}",
                                numbered.file_name().and_then(|n| n.to_str()).unwrap_or("")
                            ),
                            Some("info"),
                        );
                        let mut retry_args = sp_args.clone();
                        retry_args.retain(|a| a != "--no-overwrites");
                        if let Some(o_idx) = retry_args.iter().position(|a| a == "-o") {
                            retry_args[o_idx + 1] = numbered.to_string_lossy().to_string();
                        }
                        let _ = run_proc(
                            &ytdlp_bin,
                            &retry_args,
                            &dir,
                            &prefix,
                            idx,
                            total,
                            job_arc.cancelled.clone(),
                            job_arc.child_pid.clone(),
                            &app,
                        )
                        .await;
                        if job_arc.cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        send_item(idx, "done", "done \u{2713}");
                        send_log(&format!("Saved to {}", dir), Some("success"));
                    } else if result.code == 0 {
                        send_item(idx, "done", "done \u{2713}");
                        send_log(&format!("Saved to {}", dir), Some("success"));
                    } else {
                        send_item(idx, "error", "error \u{2717}");
                        send_log(&format!("Failed (exit {})", result.code), Some("error"));
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
        } else {
            send_item(idx, "active", "downloading\u{2026}");
            send_log(
                &format!("\u{2193} [{}/{}] {}", idx + 1, total, url),
                Some("info"),
            );

            let mut args = build_args(&prefix, &url, &dir, opts, &ffmpeg_bin, None);
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

                    if let Some(skipped) = result.skipped_path {
                        let numbered = get_available_path(Path::new(&skipped));
                        send_log(
                            &format!(
                                "File exists \u{2014} saving as: {}",
                                numbered.file_name().and_then(|n| n.to_str()).unwrap_or("")
                            ),
                            Some("info"),
                        );
                        let retry_args = build_args(
                            &prefix,
                            &url,
                            &dir,
                            opts,
                            &ffmpeg_bin,
                            Some(&numbered.to_string_lossy()),
                        );
                        let _ = run_proc(
                            &ytdlp_bin,
                            &retry_args,
                            &dir,
                            &prefix,
                            idx,
                            total,
                            job_arc.cancelled.clone(),
                            job_arc.child_pid.clone(),
                            &app,
                        )
                        .await;
                        if job_arc.cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        send_item(idx, "done", "done \u{2713}");
                        send_log(&format!("Saved to {}", dir), Some("success"));
                    } else if result.code == 0 {
                        send_item(idx, "done", "done \u{2713}");
                        send_log(&format!("Saved to {}", dir), Some("success"));
                    } else {
                        send_item(idx, "error", "error \u{2717}");
                        send_log(&format!("Failed (exit {})", result.code), Some("error"));
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
