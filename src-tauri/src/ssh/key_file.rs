//! Native "choose a private key" dialog for the session and bookmark editors.
//!
//! The webview's own `<input type="file">` cannot be told where to start, and
//! on macOS and Linux its dialog hides dot-directories — so `~/.ssh`, where
//! nearly every key lives, could not be reached without knowing the platform's
//! "show hidden files" shortcut. This command opens the dialog *inside* that
//! directory instead, and reads the pick itself, so the webview is handed one
//! file the user chose rather than a way to read arbitrary paths.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use super::transfer::home_dir;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedPrivateKeyFile {
    /// File name only, for display — the full path stays on this side.
    pub name: String,
    /// `None` when the file is larger than the caller's limit; it is not read
    /// past that point.
    pub content: Option<String>,
}

/// Where the dialog starts: `~/.ssh` when there is one, else the home
/// directory, so a machine with no keys yet does not land somewhere arbitrary.
fn default_key_dir(home: &Path) -> PathBuf {
    let ssh_dir = home.join(".ssh");
    if ssh_dir.is_dir() {
        ssh_dir
    } else {
        home.to_path_buf()
    }
}

/// Reads at most `max_bytes` of `path`. Counts bytes as they are read rather
/// than trusting the reported length, which is 0 for devices and pipes.
fn read_key_file(path: &Path, max_bytes: u64) -> Result<PickedPrivateKeyFile, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = File::open(path).map_err(|error| format!("Failed to open {name}: {error}"))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read {name}: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Ok(PickedPrivateKeyFile { name, content: None });
    }
    // Lossy, so a binary mis-pick comes back as text the caller can reject as
    // "not a key" instead of as a read failure.
    let content = String::from_utf8_lossy(&bytes).into_owned();
    Ok(PickedPrivateKeyFile { name, content: Some(content) })
}

/// Lets the user choose a private key file. `Ok(None)` when they cancel.
#[tauri::command]
pub async fn ssh_pick_private_key_file(
    app: AppHandle,
    window: WebviewWindow,
    max_bytes: u64,
) -> Result<Option<PickedPrivateKeyFile>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_directory(default_key_dir(&home_dir()))
        .set_parent(&window)
        .pick_file(move |picked| {
            let _ = sender.send(picked);
        });
    let Some(picked) = receiver
        .await
        .map_err(|_| "The file dialog closed unexpectedly".to_string())?
    else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|error| format!("Unsupported file location: {error}"))?;
    tokio::task::spawn_blocking(move || read_key_file(&path, max_bytes))
        .await
        .map_err(|error| format!("Failed to read the private key file: {error}"))?
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::{default_key_dir, read_key_file, PickedPrivateKeyFile};
    use std::fs;
    use std::path::PathBuf;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("auraterm-key-file-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn dialog_starts_in_the_ssh_directory_when_there_is_one() {
        let home = TempDir::new("with-ssh");
        fs::create_dir(home.0.join(".ssh")).unwrap();
        assert_eq!(default_key_dir(&home.0), home.0.join(".ssh"));
    }

    #[test]
    fn dialog_falls_back_to_home_without_an_ssh_directory() {
        let home = TempDir::new("no-ssh");
        assert_eq!(default_key_dir(&home.0), home.0);

        // A stray *file* called `.ssh` is not somewhere a dialog can open.
        fs::write(home.0.join(".ssh"), "").unwrap();
        assert_eq!(default_key_dir(&home.0), home.0);
    }

    #[test]
    fn reads_a_key_and_reports_only_its_file_name() {
        let dir = TempDir::new("read");
        let path = dir.0.join("id_ed25519");
        fs::write(&path, "-----BEGIN OPENSSH PRIVATE KEY-----\n").unwrap();
        assert_eq!(
            read_key_file(&path, 64),
            Ok(PickedPrivateKeyFile {
                name: "id_ed25519".to_string(),
                content: Some("-----BEGIN OPENSSH PRIVATE KEY-----\n".to_string()),
            }),
        );
    }

    #[test]
    fn withholds_the_content_of_a_file_over_the_limit() {
        let dir = TempDir::new("limit");
        let path = dir.0.join("disk.img");

        fs::write(&path, "x".repeat(8)).unwrap();
        assert_eq!(read_key_file(&path, 8).unwrap().content, Some("x".repeat(8)));

        fs::write(&path, "x".repeat(9)).unwrap();
        assert_eq!(
            read_key_file(&path, 8),
            Ok(PickedPrivateKeyFile { name: "disk.img".to_string(), content: None }),
        );
    }

    #[test]
    fn returns_binary_content_as_text_rather_than_failing() {
        let dir = TempDir::new("binary");
        let path = dir.0.join("photo.jpg");
        fs::write(&path, [0xff, 0xd8, 0xff, 0xe0]).unwrap();
        assert!(read_key_file(&path, 64).unwrap().content.is_some());
    }

    #[test]
    fn reports_a_file_it_cannot_open() {
        let dir = TempDir::new("missing");
        let error = read_key_file(&dir.0.join("gone"), 64).unwrap_err();
        assert!(error.starts_with("Failed to open gone:"), "{error}");
    }
}
