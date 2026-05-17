mod setup;
mod download;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use download::{CurrentJob, DownloadItem};

pub struct AppState {
    pub ytdlp_bin: Mutex<Option<String>>,
    pub ffmpeg_bin: std::sync::Mutex<String>,
    // One job slot per platform prefix ("yt", "ig") so each platform can run concurrently.
    pub jobs: Arc<Mutex<HashMap<String, Arc<CurrentJob>>>>,
}

fn find_ffmpeg(app: &AppHandle) -> String {
    // 1. Bundled resource
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join("ffmpeg");
        if bundled.exists() {
            return bundled.to_string_lossy().to_string();
        }
    }

    // 2. Homebrew
    let homebrew = "/opt/homebrew/bin/ffmpeg";
    if std::path::Path::new(homebrew).exists() {
        return homebrew.to_string();
    }

    // 3. which ffmpeg
    if let Ok(output) = std::process::Command::new("which").arg("ffmpeg").output() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return path;
        }
    }

    "ffmpeg".to_string()
}

// ── Commands ──────────────────────────────────────────────────

#[tauri::command]
async fn get_initial_state(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if setup::is_setup_complete(&app_data) {
        let bin_path = setup::get_ytdlp_bin_path(&app_data);
        *state.ytdlp_bin.lock().await = Some(bin_path.to_string_lossy().to_string());
        Ok(serde_json::json!({ "screen": "main" }))
    } else {
        Ok(serde_json::json!({ "screen": "setup" }))
    }
}

#[tauri::command]
async fn setup_start(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    match setup::run_setup(&app, &app_data).await {
        Ok((bin_path, version)) => {
            *state.ytdlp_bin.lock().await = Some(bin_path.to_string_lossy().to_string());
            Ok(serde_json::json!({ "success": true, "version": version }))
        }
        Err(e) => Ok(serde_json::json!({ "success": false, "error": e })),
    }
}

#[tauri::command]
async fn ytdlp_check(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let bin = state.ytdlp_bin.lock().await.clone();
    match bin {
        None => Ok(serde_json::json!({ "found": false, "version": "" })),
        Some(bin_path) => {
            match tokio::process::Command::new(&bin_path)
                .arg("--version")
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    Ok(serde_json::json!({ "found": true, "version": v }))
                }
                _ => Ok(serde_json::json!({ "found": false, "version": "" })),
            }
        }
    }
}

#[tauri::command]
async fn ytdlp_do_update(
    app: AppHandle,
    state: State<'_, AppState>,
    download_url: String,
) -> Result<serde_json::Value, String> {
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    match setup::perform_update(&app, &app_data, &download_url).await {
        Ok(()) => {
            let bin_path = setup::get_ytdlp_bin_path(&app_data);
            *state.ytdlp_bin.lock().await = Some(bin_path.to_string_lossy().to_string());
            Ok(serde_json::json!({ "success": true }))
        }
        Err(e) => Ok(serde_json::json!({ "success": false, "error": e })),
    }
}

#[tauri::command]
async fn dialog_folder(app: AppHandle) -> Result<Option<String>, String> {
    let result = app
        .dialog()
        .file()
        .set_title("Select download folder")
        .blocking_pick_folder();

    Ok(result.map(|p| p.to_string()))
}

#[tauri::command]
async fn shell_open_folder(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
async fn download_start(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: serde_json::Value,
) -> Result<(), String> {
    let prefix = payload["prefix"]
        .as_str()
        .ok_or("missing prefix")?
        .to_string();
    let dir = payload["dir"]
        .as_str()
        .unwrap_or("~/Downloads/Ingest")
        .to_string();
    let dir = if dir.starts_with('~') {
        let home = dirs_home();
        dir.replacen('~', &home, 1)
    } else {
        dir
    };

    let items_val = payload["items"]
        .as_array()
        .ok_or("missing items")?;
    let items: Vec<DownloadItem> = items_val
        .iter()
        .filter_map(|v| {
            let url = v["url"].as_str()?.trim().to_string();
            if url.is_empty() {
                return None;
            }
            let opts = v["opts"].clone();
            Some(DownloadItem { url, opts })
        })
        .collect();

    if items.is_empty() {
        let _ = app.emit(
            "download:log",
            serde_json::json!({ "prefix": prefix, "text": "No URLs queued.", "type": "warn" }),
        );
        let _ = app.emit(
            "download:complete",
            serde_json::json!({ "prefix": prefix, "success": false }),
        );
        return Ok(());
    }

    // Cancel any existing job for this prefix only — other platforms keep running.
    {
        let mut jobs = state.jobs.lock().await;
        if let Some(old_job) = jobs.remove(&prefix) {
            old_job.cancelled.store(true, Ordering::Relaxed);
            let pid = *old_job.child_pid.lock().await;
            if let Some(p) = pid {
                download::kill_child(p);
            }
        }
    }

    let ytdlp_bin = state
        .ytdlp_bin
        .lock()
        .await
        .clone()
        .ok_or("yt-dlp not installed")?;
    let ffmpeg_bin = state.ffmpeg_bin.lock().unwrap().clone();

    let job = Arc::new(CurrentJob {
        prefix: prefix.clone(),
        queue: Arc::new(Mutex::new(vec![])),
        cancelled: Arc::new(AtomicBool::new(false)),
        child_pid: Arc::new(Mutex::new(None)),
    });

    state.jobs.lock().await.insert(prefix.clone(), job.clone());

    let app_clone  = app.clone();
    let job_arc    = job.clone();
    let jobs_clone = state.jobs.clone();
    let prefix_done = prefix.clone();

    tokio::spawn(async move {
        download::run_download_job(
            app_clone,
            ytdlp_bin,
            ffmpeg_bin,
            prefix,
            items,
            dir,
            job_arc,
        )
        .await;
        // Remove completed job so download_enqueue doesn't hot-enqueue into a dead job.
        jobs_clone.lock().await.remove(&prefix_done);
    });

    Ok(())
}

#[tauri::command]
async fn download_cancel(state: State<'_, AppState>, prefix: String) -> Result<(), String> {
    let mut jobs = state.jobs.lock().await;
    if let Some(job) = jobs.remove(&prefix) {
        job.cancelled.store(true, Ordering::Relaxed);
        let pid = *job.child_pid.lock().await;
        if let Some(p) = pid {
            download::kill_child(p);
        }
    }
    Ok(())
}

#[tauri::command]
async fn download_enqueue(
    state: State<'_, AppState>,
    prefix: String,
    items: Vec<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let jobs = state.jobs.lock().await;
    match jobs.get(&prefix) {
        Some(job) => {
            let new_items: Vec<DownloadItem> = items
                .iter()
                .filter_map(|v| {
                    let url = v["url"].as_str()?.trim().to_string();
                    if url.is_empty() { return None; }
                    let opts = v["opts"].clone();
                    Some(DownloadItem { url, opts })
                })
                .collect();
            let mut q = job.queue.lock().await;
            q.extend(new_items);
            Ok(serde_json::json!({ "enqueued": true }))
        }
        None => Ok(serde_json::json!({ "enqueued": false })),
    }
}

#[tauri::command]
async fn format_detect(
    state: State<'_, AppState>,
    url: String,
) -> Result<serde_json::Value, String> {
    let bin = state.ytdlp_bin.lock().await.clone();
    let bin = match bin {
        None => return Ok(serde_json::json!({ "height": 0, "abr": 0 })),
        Some(b) => b,
    };

    let output = tokio::process::Command::new(&bin)
        .args(["-j", "--no-warnings", "--no-playlist", &url])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut max_height: u32 = 0;
    let mut max_abr: u32 = 0;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(line) {
            let formats = if let Some(arr) = data["formats"].as_array() {
                arr.clone()
            } else {
                vec![data.clone()]
            };
            for f in formats {
                if let Some(h) = f["height"].as_u64() {
                    max_height = max_height.max(h as u32);
                }
                if let Some(a) = f["abr"].as_f64() {
                    max_abr = max_abr.max(a as u32);
                }
            }
        }
    }

    Ok(serde_json::json!({ "height": max_height, "abr": max_abr }))
}

async fn check_app_update_inner(app: &AppHandle) -> serde_json::Value {
    let current = app.package_info().version.to_string();

    let client = match reqwest::Client::builder().user_agent("ingest-app").build() {
        Ok(c) => c,
        Err(_) => return serde_json::json!({ "hasUpdate": false }),
    };

    let resp = match client
        .get("https://api.github.com/repos/sarwa-13/Ingest/releases/latest")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return serde_json::json!({ "hasUpdate": false }),
    };

    if !resp.status().is_success() {
        return serde_json::json!({ "hasUpdate": false });
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(_) => return serde_json::json!({ "hasUpdate": false }),
    };

    let latest_tag = body["tag_name"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches('v')
        .to_string();
    let download_url = body["html_url"].as_str().unwrap_or("").to_string();

    let has_update = !latest_tag.is_empty() && latest_tag != current;

    serde_json::json!({
        "hasUpdate": has_update,
        "latestVersion": latest_tag,
        "currentVersion": current,
        "downloadUrl": download_url
    })
}

#[tauri::command]
async fn app_check_update(app: AppHandle) -> Result<serde_json::Value, String> {
    Ok(check_app_update_inner(&app).await)
}

#[tauri::command]
async fn app_download_update(url: String) -> Result<(), String> {
    tauri_plugin_opener::open_url(&url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
async fn app_install_update() -> Result<(), String> {
    Ok(())
}

fn dirs_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            ytdlp_bin: Mutex::new(None),
            ffmpeg_bin: std::sync::Mutex::new(String::new()),
            jobs: Arc::new(Mutex::new(HashMap::new())),
        })
        .setup(|app| {
            let ffmpeg = find_ffmpeg(app.handle());
            let state = app.state::<AppState>();
            *state.ffmpeg_bin.lock().unwrap() = ffmpeg;

            // Schedule yt-dlp update check 4s after launch
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;

                // App self-update check (always runs)
                let update_info = check_app_update_inner(&app_handle).await;
                if update_info["hasUpdate"].as_bool() == Some(true) {
                    let _ = app_handle.emit("app:update-available", update_info);
                }

                // yt-dlp update check (only when app is fully set up)
                let app_data = match app_handle.path().app_data_dir() {
                    Ok(p) => p,
                    Err(_) => return,
                };
                if !setup::is_setup_complete(&app_data) {
                    return;
                }
                match setup::check_for_update(&app_data).await {
                    Ok(info) if info["hasUpdate"].as_bool() == Some(true) => {
                        let _ = app_handle.emit("ytdlp:update-available", info);
                    }
                    _ => {}
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_initial_state,
            setup_start,
            ytdlp_check,
            ytdlp_do_update,
            dialog_folder,
            shell_open_folder,
            download_start,
            download_cancel,
            download_enqueue,
            format_detect,
            app_check_update,
            app_download_update,
            app_install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
