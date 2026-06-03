use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

/// Configuration sent from the frontend form. Field names match the JS object
/// assembled in `src/main.ts`.
#[derive(Debug, Deserialize)]
pub struct RenameConfig {
    image_folder: String,
    /// When set, renamed files are *copied* here and originals are left in place.
    /// When `None`, files are renamed in place.
    output_folder: Option<String>,
    filename_prefix: String,
    server_url: String,
    prompt: String,
    max_tokens: i64,
    /// Lower-cased extensions without the dot, e.g. ["jpg", "jpeg"].
    extensions: Vec<String>,
}

/// One progress update, emitted on the `renamer://progress` event channel.
#[derive(Debug, Clone, Serialize)]
struct ProgressEvent {
    done: usize,
    total: usize,
    file: String,
    /// "info" | "renamed" | "copied" | "skipped" | "error" | "summary"
    status: String,
    message: String,
}

const PROGRESS_EVENT: &str = "renamer://progress";

#[derive(Default)]
pub struct AppState {
    cancel: Arc<AtomicBool>,
}

fn emit(app: &AppHandle, done: usize, total: usize, file: &str, status: &str, message: &str) {
    let _ = app.emit(
        PROGRESS_EVENT,
        ProgressEvent {
            done,
            total,
            file: file.to_string(),
            status: status.to_string(),
            message: message.to_string(),
        },
    );
}

/// Port of the original shell `sed` pipeline: replace anything outside
/// `[A-Za-z0-9_.-]` with `_`, collapse runs of `_`, and trim leading/trailing `_`.
fn sanitize(input: &str) -> String {
    let replaced: String = input
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Collapse consecutive underscores.
    let mut collapsed = String::with_capacity(replaced.len());
    let mut prev_underscore = false;
    for c in replaced.chars() {
        if c == '_' {
            if !prev_underscore {
                collapsed.push(c);
            }
            prev_underscore = true;
        } else {
            collapsed.push(c);
            prev_underscore = false;
        }
    }

    collapsed.trim_matches('_').to_string()
}

fn mime_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
}

/// Derive the `/v1/models` URL from the configured chat-completions URL.
fn derive_models_url(chat_url: &str) -> String {
    if chat_url.contains("/chat/completions") {
        chat_url.replace("/chat/completions", "/models")
    } else if let Ok(url) = reqwest::Url::parse(chat_url) {
        format!("{}/v1/models", url.origin().ascii_serialization())
    } else {
        chat_url.to_string()
    }
}

/// Ask the server which models are loaded and return the first one's id.
/// LM Studio normally has a single model loaded at a time.
async fn first_model(client: &reqwest::Client, models_url: &str) -> Result<String, String> {
    let resp = client
        .get(models_url)
        .send()
        .await
        .map_err(|e| format!("could not reach model list at {models_url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("model list returned {}", resp.status()));
    }
    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid model list JSON: {e}"))?;
    value["data"][0]["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "no models are loaded on the server".to_string())
}

/// Whether two paths refer to the same existing file (by canonical path).
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Find a free destination path: `<stem>.<ext>`, then `<stem>_1.<ext>`,
/// `<stem>_2.<ext>`, … The source's own path counts as free, so a file that is
/// already correctly named is not bumped to a `_1` variant.
fn next_available(dir: &Path, stem: &str, ext: &str, source: &Path) -> PathBuf {
    let mut n = 0usize;
    loop {
        let name = if n == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}_{n}.{ext}")
        };
        let candidate = dir.join(&name);
        if !candidate.exists() || same_path(&candidate, source) {
            return candidate;
        }
        n += 1;
    }
}

/// Collect matching image files from the folder, sorted by name for stable order.
fn collect_images(folder: &Path, extensions: &[String]) -> std::io::Result<Vec<PathBuf>> {
    let wanted: Vec<String> = extensions.iter().map(|e| e.to_ascii_lowercase()).collect();
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| wanted.contains(&e.to_ascii_lowercase()))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Send one image to the LLM and return the raw text answer.
async fn query_llm(
    client: &reqwest::Client,
    config: &RenameConfig,
    model: &str,
    image_path: &Path,
) -> Result<String, String> {
    let bytes = std::fs::read(image_path).map_err(|e| format!("read failed: {e}"))?;
    let b64 = STANDARD.encode(&bytes);
    let ext = image_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg");
    let data_url = format!("data:{};base64,{}", mime_for(ext), b64);

    // OpenAI Chat Completions content parts (what LM Studio expects). Note this
    // differs from the original script, which used the Responses-API field names.
    let payload = json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": config.prompt },
                { "type": "image_url", "image_url": { "url": data_url } }
            ]
        }],
        "max_tokens": config.max_tokens,
        "stream": false
    });

    let resp = client
        .post(&config.server_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("server returned {status}: {}", body.trim()));
    }

    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON response: {e}"))?;

    value["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "no content in response".to_string())
}

#[tauri::command]
async fn run_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    config: RenameConfig,
) -> Result<(), String> {
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::SeqCst);

    let folder = PathBuf::from(&config.image_folder);
    if !folder.is_dir() {
        return Err(format!("Image folder not found: {}", config.image_folder));
    }

    // Resolve destination directory + whether we copy (output set) or rename.
    let (dest_dir, copy_mode) = match &config.output_folder {
        Some(out) if !out.is_empty() => {
            let out_path = PathBuf::from(out);
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("could not create output folder: {e}"))?;
            (out_path, true)
        }
        _ => (folder.clone(), false),
    };

    let client = reqwest::Client::new();

    // Use whichever model the server currently has loaded.
    let models_url = derive_models_url(&config.server_url);
    let model = first_model(&client, &models_url).await?;
    emit(&app, 0, 0, "", "info", &format!("Using model: {model}"));

    let images = collect_images(&folder, &config.extensions)
        .map_err(|e| format!("could not read folder: {e}"))?;
    let total = images.len();
    emit(&app, 0, total, "", "info", &format!("Found {total} image(s)"));

    let mut renamed = 0usize;
    let mut copied = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut cancelled = false;

    for (idx, image_path) in images.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }

        let done = idx + 1;
        let name = image_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        emit(&app, idx, total, &name, "info", "processing");

        let answer = match query_llm(&client, &config, &model, image_path).await {
            Ok(a) => a,
            Err(e) => {
                errors += 1;
                emit(&app, done, total, &name, "error", &e);
                continue;
            }
        };

        let sanitized = sanitize(&answer);
        if sanitized.is_empty() {
            skipped += 1;
            emit(&app, done, total, &name, "skipped", "empty result after sanitizing");
            continue;
        }

        let ext = image_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg");
        let stem = format!("{}{}", config.filename_prefix, sanitized);
        // On a name clash with a different file, fall back to <stem>_1, _2, …
        let dest = next_available(&dest_dir, &stem, ext, image_path);
        let new_name = dest
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // The file is already correctly named (destination resolves to itself).
        if same_path(&dest, image_path) {
            skipped += 1;
            emit(&app, done, total, &name, "skipped", &format!("already named {new_name}"));
            continue;
        }

        if copy_mode {
            match std::fs::copy(image_path, &dest) {
                Ok(_) => {
                    copied += 1;
                    emit(&app, done, total, &name, "copied", &new_name);
                }
                Err(e) => {
                    errors += 1;
                    emit(&app, done, total, &name, "error", &format!("copy failed: {e}"));
                }
            }
        } else {
            match std::fs::rename(image_path, &dest) {
                Ok(_) => {
                    renamed += 1;
                    emit(&app, done, total, &name, "renamed", &new_name);
                }
                Err(e) => {
                    errors += 1;
                    emit(&app, done, total, &name, "error", &format!("rename failed: {e}"));
                }
            }
        }
    }

    let done = if cancelled { renamed + copied + skipped + errors } else { total };
    let verb = if cancelled { "Cancelled" } else { "Complete" };
    let summary = format!(
        "{verb}: {renamed} renamed, {copied} copied, {skipped} skipped, {errors} error(s)."
    );
    emit(&app, done, total, "", "summary", &summary);
    Ok(())
}

#[tauri::command]
fn cancel_rename(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![run_rename, cancel_rename])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
