use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ICLOUD_RECOVERY_URL: &str = "https://www.icloud.com/recovery/";
pub const DEFAULT_CLIENT_BUILD: &str = "2546Build54";
pub const DEFAULT_RESTORE_BATCH_SIZE: usize = 100;
pub const DEFAULT_FETCH_PAGE_SIZE: usize = 2_000;
pub const DEFAULT_MAX_RETRIES: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    pub cookies: String,
    pub client_id: String,
    pub dsid: String,
    pub client_build_number: String,
    pub client_mastering_number: String,
}

impl Credentials {
    pub fn new(cookies: String, client_id: String, dsid: String) -> Self {
        Self {
            cookies,
            client_id,
            dsid,
            client_build_number: DEFAULT_CLIENT_BUILD.to_string(),
            client_mastering_number: DEFAULT_CLIENT_BUILD.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeletedItem {
    pub item_id: String,
    pub name: Option<String>,
    pub item_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreStats {
    pub total: usize,
    pub restored: usize,
    pub failed: usize,
    pub failed_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RestoreEvent {
    Status { message: String },
    AuthStarted,
    Authenticated,
    ScanProgress { page: u64, page_count: usize, total: usize },
    ScanComplete { total: usize },
    RestoreStarted { total: usize },
    RestoreProgress {
        total: usize,
        restored: usize,
        failed: usize,
        eta_seconds: Option<u64>,
        message: String,
    },
    Retry { batch_number: usize, attempt: usize, message: String },
    Paused { message: String },
    Complete { stats: RestoreStats },
    Error { message: String, recoverable: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AppPhase {
    Welcome,
    SigningIn,
    ReadyToScan,
    Scanning,
    ReadyToRestore,
    Restoring,
    Paused,
    Complete,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub phase: AppPhase,
    pub deleted_count: usize,
    pub stats: RestoreStats,
    pub message: String,
    pub can_resume: bool,
}

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("Chrome is needed for secure Apple sign-in. Install Chrome, then try again.")]
    ChromeMissing,
    #[error("Chrome did not open. Close any stuck Chrome windows, then try again.")]
    ChromeLaunchFailed,
    #[error("We could not connect to Chrome for sign-in.")]
    ChromeConnectionFailed,
    #[error("We could not detect a completed iCloud sign-in.")]
    LoginTimeout,
    #[error("iCloud needs you to sign in again.")]
    AuthExpired,
    #[error("Saved progress could not be read: {0}")]
    ProgressCorrupt(String),
    #[error("iCloud request failed: {0}")]
    Api(String),
    #[error("Network request failed: {0}")]
    Network(String),
    #[error("File operation failed: {0}")]
    File(String),
    #[error("No active iCloud session. Sign in again to continue.")]
    MissingCredentials,
    #[error("No deleted items have been scanned yet.")]
    MissingScan,
    #[error("Restore was paused and progress was saved.")]
    Cancelled,
}

impl From<reqwest::Error> for RestoreError {
    fn from(value: reqwest::Error) -> Self {
        if let Some(status) = value.status() {
            if matches!(status.as_u16(), 401 | 403 | 421) {
                return Self::AuthExpired;
            }
        }
        Self::Network(value.to_string())
    }
}
