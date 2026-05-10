use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};
use serde_json::Value;
use futures_util::StreamExt;

const YTDLP_REPO: &str = "yt-dlp/yt-dlp";

fn get_asset_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "yt-dlp_macos"
    } else if cfg!(target_os = "windows") {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    }
}

pub fn get_bin_dir(app_data: &Path) -> PathBuf {
    app_data.join("bin")
}

pub fn get_ytdlp_bin_path(app_data: &Path) -> PathBuf {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    get_bin_dir(app_data).join(format!("yt-dlp{}", ext))
}

pub fn get_version_path(app_data: &Path) -> PathBuf {
    get_bin_dir(app_data).join("yt-dlp-version.txt")
}

pub fn is_setup_complete(app_data: &Path) -> bool {
    let bin = get_ytdlp_bin_path(app_data);
    if !bin.exists() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&bin) {
            return meta.permissions().mode() & 0o111 != 0;
        }
        return false;
    }
    #[cfg(not(unix))]
    {
        return true;
    }
}

async fn fetch_release(client: &reqwest::Client) -> Result<Value, String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", YTDLP_REPO);
    let resp = client
        .get(&url)
        .header("User-Agent", "Ingest/1.0")
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(json)
}

fn fmt_bytes(b: u64) -> String {
    if b < 1_048_576 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / 1_048_576.0)
    }
}

pub async fn run_setup(app: &AppHandle, app_data: &Path) -> Result<(PathBuf, String), String> {
    let bin_dir = get_bin_dir(app_data);
    std::fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;

    let _ = app.emit("setup:progress", serde_json::json!({ "label": "Fetching latest yt-dlp release…", "pct": 2 }));

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;

    let release = fetch_release(&client).await?;
    let tag = release["tag_name"].as_str().unwrap_or("unknown").to_string();

    let asset_name = get_asset_name();
    let assets = release["assets"].as_array().ok_or("No assets in release")?;
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(asset_name))
        .ok_or_else(|| format!("Asset not found: {}", asset_name))?;

    let download_url = asset["browser_download_url"]
        .as_str()
        .ok_or("No browser_download_url")?
        .to_string();

    let bin_path = get_ytdlp_bin_path(app_data);

    download_file_with_progress(&client, &download_url, &bin_path, {
        let app = app.clone();
        let tag = tag.clone();
        move |pct, downloaded, total| {
            let label = format!(
                "Downloading yt-dlp {}  {} / {}",
                tag,
                fmt_bytes(downloaded),
                fmt_bytes(total)
            );
            let progress_pct = 5 + (pct as f64 * 0.9) as u32;
            let _ = app.emit("setup:progress", serde_json::json!({ "label": label, "pct": progress_pct }));
        }
    })
    .await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).map_err(|e| e.to_string())?;
    }

    std::fs::write(get_version_path(app_data), &tag).map_err(|e| e.to_string())?;

    let _ = app.emit("setup:progress", serde_json::json!({ "label": "All done — launching Ingest…", "pct": 100 }));

    Ok((bin_path, tag))
}

pub async fn check_for_update(app_data: &Path) -> Result<Value, String> {
    let version_path = get_version_path(app_data);
    let current = if version_path.exists() {
        std::fs::read_to_string(&version_path)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        String::new()
    };

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;

    let release = fetch_release(&client).await?;
    let latest = release["tag_name"].as_str().unwrap_or("").to_string();

    if !current.is_empty() && !latest.is_empty() && current != latest {
        let asset_name = get_asset_name();
        let assets = release["assets"].as_array();
        let download_url = assets
            .and_then(|arr| arr.iter().find(|a| a["name"].as_str() == Some(asset_name)))
            .and_then(|a| a["browser_download_url"].as_str())
            .unwrap_or("")
            .to_string();

        return Ok(serde_json::json!({
            "hasUpdate": true,
            "currentVersion": current,
            "latestVersion": latest,
            "downloadUrl": download_url,
        }));
    }

    Ok(serde_json::json!({
        "hasUpdate": false,
        "currentVersion": current,
        "latestVersion": latest,
    }))
}

pub async fn perform_update(
    app: &AppHandle,
    app_data: &Path,
    download_url: &str,
) -> Result<(), String> {
    let bin_path = get_ytdlp_bin_path(app_data);
    let tmp_path = bin_path.with_extension("tmp");

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;

    download_file_with_progress(&client, download_url, &tmp_path, {
        let app = app.clone();
        move |pct, _dl, _total| {
            let _ = app.emit("ytdlp:update-progress", serde_json::json!({ "pct": pct }));
        }
    })
    .await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp_path)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp_path, perms).map_err(|e| e.to_string())?;
    }

    #[cfg(unix)]
    std::fs::rename(&tmp_path, &bin_path).map_err(|e| e.to_string())?;

    #[cfg(not(unix))]
    {
        let _ = std::fs::remove_file(&bin_path);
        std::fs::rename(&tmp_path, &bin_path).map_err(|e| e.to_string())?;
    }

    // Try to update version file
    if let Ok(release) = fetch_release(&client).await {
        if let Some(tag) = release["tag_name"].as_str() {
            let _ = std::fs::write(get_version_path(app_data), tag);
        }
    }

    Ok(())
}

async fn download_file_with_progress<F>(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    on_progress: F,
) -> Result<(), String>
where
    F: Fn(u32, u64, u64),
{
    let resp = client
        .get(url)
        .header("User-Agent", "Ingest/1.0")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| e.to_string())?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
        if total > 0 {
            let pct = (downloaded * 100 / total) as u32;
            on_progress(pct, downloaded, total);
        }
    }

    Ok(())
}
