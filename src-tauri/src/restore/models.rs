use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

pub const ICLOUD_RECOVERY_URL: &str = "https://www.icloud.com/recovery/";
/// Last resort if Chrome never emits a `-docws.icloud.com` request during capture.
pub const DOCWS_FALLBACK_ORIGIN: &str = "https://p107-docws.icloud.com";
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
    /// `https://pNNN-docws.icloud.com` from Chrome DevTools capture; omit to use [`DOCWS_FALLBACK_ORIGIN`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docws_base_url: Option<String>,
}

impl Credentials {
    pub fn new(cookies: String, client_id: String, dsid: String) -> Self {
        Self {
            cookies,
            client_id,
            dsid,
            client_build_number: DEFAULT_CLIENT_BUILD.to_string(),
            client_mastering_number: DEFAULT_CLIENT_BUILD.to_string(),
            docws_base_url: None,
        }
    }

    /// DocWS HTTPS origin for tombstone/list and restore PUT calls.
    pub fn resolved_docws_base_url(&self) -> &str {
        match self.docws_base_url.as_deref() {
            Some(s) if !s.trim().is_empty() => s.trim_end_matches('/'),
            _ => DOCWS_FALLBACK_ORIGIN.trim_end_matches('/'),
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

/// Cooperative cancellation for long-running scan / restore loops.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiMessage {
    pub id: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

impl UiMessage {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            params: BTreeMap::new(),
        }
    }

    pub fn with_param(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.params.insert(key.into(), value.to_string());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RestoreEvent {
    Status {
        message: UiMessage,
    },
    AuthStarted,
    Authenticated,
    ScanProgress {
        page: u64,
        page_count: usize,
        total: usize,
    },
    ScanComplete {
        total: usize,
    },
    ScanPaused {
        partial_total: usize,
    },
    RestoreStarted {
        total: usize,
    },
    RestoreProgress {
        total: usize,
        restored: usize,
        failed: usize,
        eta_seconds: Option<u64>,
        message: UiMessage,
    },
    Retry {
        batch_number: usize,
        attempt: usize,
        message: UiMessage,
    },
    Paused {
        message: UiMessage,
    },
    Complete {
        stats: RestoreStats,
    },
    Error {
        message: UiMessage,
        recoverable: bool,
    },
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
    pub message: UiMessage,
    pub can_resume: bool,
}

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("error.chromeMissing")]
    ChromeMissing,
    #[error("error.chromeLaunchFailed")]
    ChromeLaunchFailed,
    #[error("error.chromeConnectionFailed")]
    ChromeConnectionFailed,
    #[error("error.loginTimeout")]
    LoginTimeout,
    #[error("error.authExpired")]
    AuthExpired,
    #[error("error.progressCorrupt")]
    ProgressCorrupt(String),
    #[error("error.api")]
    Api(String),
    #[error("error.network")]
    Network(String),
    #[error("error.file")]
    File(String),
    #[error("error.missingCredentials")]
    MissingCredentials,
    #[error("error.missingScan")]
    MissingScan,
    #[error("error.cancelled")]
    Cancelled,
    #[error("status.scanCancelledSave")]
    ScanCancelled,
}

impl RestoreError {
    pub fn message(&self) -> UiMessage {
        match self {
            Self::ChromeMissing => UiMessage::new("error.chromeMissing"),
            Self::ChromeLaunchFailed => UiMessage::new("error.chromeLaunchFailed"),
            Self::ChromeConnectionFailed => UiMessage::new("error.chromeConnectionFailed"),
            Self::LoginTimeout => UiMessage::new("error.loginTimeout"),
            Self::AuthExpired => UiMessage::new("error.authExpired"),
            Self::ProgressCorrupt(details) => {
                UiMessage::new("error.progressCorrupt").with_param("details", details)
            }
            Self::Api(details) => UiMessage::new("error.api").with_param("details", details),
            Self::Network(details) => {
                UiMessage::new("error.network").with_param("details", details)
            }
            Self::File(details) => UiMessage::new("error.file").with_param("details", details),
            Self::MissingCredentials => UiMessage::new("error.missingCredentials"),
            Self::MissingScan => UiMessage::new("error.missingScan"),
            Self::Cancelled => UiMessage::new("error.cancelled"),
            Self::ScanCancelled => UiMessage::new("status.scanCancelledSave"),
        }
    }
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
