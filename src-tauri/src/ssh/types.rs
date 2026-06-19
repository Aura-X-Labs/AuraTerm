//! Plain data types shared across the `ssh` submodules.
//!
//! These types are split out from [`super`] (mod.rs) to reduce the size of the
//! main module file. They intentionally carry no behaviour — only
//! serde-serializable payloads and small enums used by both the PTY lifecycle
//! and the file-transfer code paths.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JumpHostConfig {
    pub id: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    #[serde(default = "default_password_auth")]
    pub auth_type: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub private_key: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoLoginRule {
    pub expect: String,
    #[serde(default)]
    pub response: Option<String>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_expect_timeout")]
    pub timeout_secs: u64,
}

fn default_ssh_port() -> u16 { 22 }
fn default_password_auth() -> String { "password".to_string() }
fn default_expect_timeout() -> u64 { 30 }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedSshKeyPair {
    pub private_key: String,
    pub public_key: String,
    pub fingerprint: String,
}


#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshTransferMode {
    Sftp,
    Scp,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshTransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshTransferStatus {
    Started,
    Progress,
    Completed,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTransferProgressPayload {
    pub id: String,
    pub direction: SshTransferDirection,
    pub status: SshTransferStatus,
    pub mode: SshTransferMode,
    pub file_name: String,
    pub remote_path: String,
    pub local_path: Option<String>,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: Option<u64>,
    pub permissions: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirectoryListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<RemoteFileEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SshMfaPrompt {
    pub text: String,
    pub echo: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeyboardInteractivePromptPayload {
    pub id: String,
    pub name: String,
    pub instruction: String,
    pub prompts: Vec<SshMfaPrompt>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TerminalDataPayload {
    pub id: String,
    pub data: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PtyExitPayload {
    pub id: String,
    pub message: String,
}

/// Result of writing interactive-auth input to the SSH channel.
///
/// Crate-visible so the PTY/auth submodules can share it without leaking
/// internal auth details outside the `ssh` module tree.
pub(super) enum InteractiveWriteOutcome {
    Sent,
    Dropped(String),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconnectSessionPromptPayload {
    pub id: String,
    pub tool: String,
    pub sessions: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostKeyMismatchPromptPayload {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub expected_fingerprint: String,
    pub observed_fingerprint: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedSshHostKeyEntry {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub fingerprint_summary: String,
}
