use crate::restore::api::ICloudApiClient;
use crate::restore::browser::ChromeAuthenticator;
use crate::restore::checkpoint::CheckpointStore;
use crate::restore::job::RestoreSupervisor;
use crate::restore::models::{
    AppPhase, CancellationToken, Credentials, RestoreError, RestoreEvent, RestoreStats, UiMessage,
    UiSnapshot,
};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State, Window};
use tokio::sync::Mutex;

pub struct AppState {
    session: Mutex<AppSession>,
}

#[derive(Debug)]
struct AppSession {
    phase: AppPhase,
    credentials: Option<Credentials>,
    browser: Option<ChromeAuthenticator>,
    deleted_item_ids: Vec<String>,
    /// Items counted in tombstone checkpoint when user stops mid-scan (resume with Scan again).
    partial_scan_item_count: usize,
    stats: RestoreStats,
    message: UiMessage,
    cancellation: Option<CancellationToken>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(AppSession::default()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[tauri::command]
pub async fn get_restore_state(state: State<'_, AppState>) -> Result<UiSnapshot, String> {
    let session = state.session.lock().await;
    Ok(session.snapshot(false))
}

#[tauri::command]
pub async fn start_auth(window: Window, state: State<'_, AppState>) -> Result<UiSnapshot, String> {
    let mut old_browser = {
        let mut session = state.session.lock().await;
        if let Some(token) = &session.cancellation {
            token.cancel();
        }
        let browser = session.browser.take();
        *session = AppSession {
            phase: AppPhase::SigningIn,
            message: msg("status.signingIn"),
            ..AppSession::default()
        };
        browser
    };
    if let Some(browser) = &mut old_browser {
        browser.shutdown().await;
    }

    emit_event(&window, RestoreEvent::AuthStarted)?;
    let mut browser = ChromeAuthenticator::new().map_err(to_ui_error)?;
    browser.connect_or_launch().await.map_err(to_ui_error)?;
    let credentials = browser.wait_for_login(300).await.map_err(to_ui_error)?;

    {
        let mut session = state.session.lock().await;
        session.credentials = Some(credentials);
        session.browser = Some(browser);
        session.phase = AppPhase::ReadyToScan;
        session.message = msg("status.authenticated");
    }

    emit_event(&window, RestoreEvent::Authenticated)?;
    get_restore_state(state).await
}

#[tauri::command]
pub async fn cancel_scan(state: State<'_, AppState>) -> Result<(), String> {
    let session = state.session.lock().await;
    if session.phase != AppPhase::Scanning {
        return Ok(());
    }
    if let Some(token) = &session.cancellation {
        token.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn reset_session(state: State<'_, AppState>) -> Result<UiSnapshot, String> {
    let mut old_browser = {
        let mut session = state.session.lock().await;
        if let Some(token) = &session.cancellation {
            token.cancel();
        }
        let browser = session.browser.take();
        *session = AppSession::default();
        browser
    };
    if let Some(browser) = &mut old_browser {
        browser.shutdown().await;
    }

    get_restore_state(state).await
}

#[tauri::command]
pub async fn scan_deleted_items(
    app: AppHandle,
    window: Window,
    state: State<'_, AppState>,
) -> Result<UiSnapshot, String> {
    let credentials = credentials(&state).await.map_err(to_ui_error)?;
    let store = checkpoint_store(&app)?;
    let api = ICloudApiClient::with_base_url(credentials.resolved_docws_base_url())
        .map_err(to_ui_error)?;

    let cancellation = CancellationToken::default();
    {
        let mut session = state.session.lock().await;
        session.partial_scan_item_count = 0;
        session.cancellation = Some(cancellation.clone());
        session.phase = AppPhase::Scanning;
        session.message = msg("status.scanning");
    }

    let fetch_result = api
        .fetch_deleted_items(
            &credentials,
            Some(&store),
            Some(&cancellation),
            |progress| {
                let _ = emit_event(
                    &window,
                    RestoreEvent::ScanProgress {
                        page: progress.page,
                        page_count: progress.page_count,
                        total: progress.total,
                    },
                );
            },
        )
        .await;

    {
        let mut session = state.session.lock().await;
        session.cancellation = None;
    }

    match fetch_result {
        Ok(items) => {
            let item_ids: Vec<String> = items.into_iter().map(|item| item.item_id).collect();
            {
                let mut session = state.session.lock().await;
                session.deleted_item_ids = item_ids;
                session.partial_scan_item_count = 0;
                session.phase = AppPhase::ReadyToRestore;
                session.message =
                    msg("status.scanComplete").with_param("total", session.deleted_item_ids.len());
                session.stats = RestoreStats {
                    total: session.deleted_item_ids.len(),
                    ..RestoreStats::default()
                };
            }

            emit_event(
                &window,
                RestoreEvent::ScanComplete {
                    total: state.session.lock().await.deleted_item_ids.len(),
                },
            )?;
            get_restore_state(state).await
        }
        Err(RestoreError::ScanCancelled) => {
            let partial = store
                .load_checkpoint()
                .map_err(to_ui_error)?
                .map(|checkpoint| checkpoint.item_ids.len())
                .unwrap_or(0);
            {
                let mut session = state.session.lock().await;
                session.partial_scan_item_count = partial;
                session.phase = AppPhase::ReadyToScan;
                session.message = msg("status.scanCancelledSave").with_param("count", partial);
            }

            emit_event(
                &window,
                RestoreEvent::ScanPaused {
                    partial_total: partial,
                },
            )?;
            get_restore_state(state).await
        }
        Err(error) => Err(to_ui_error(error)),
    }
}

#[tauri::command]
pub async fn start_restore(
    app: AppHandle,
    window: Window,
    state: State<'_, AppState>,
) -> Result<UiSnapshot, String> {
    let credentials = credentials(&state).await.map_err(to_ui_error)?;
    let item_ids = scanned_item_ids(&state).await.map_err(to_ui_error)?;
    run_restore(app, window, state, credentials, item_ids).await
}

#[tauri::command]
pub async fn retry_failed(
    app: AppHandle,
    window: Window,
    state: State<'_, AppState>,
) -> Result<UiSnapshot, String> {
    let credentials = credentials(&state).await.map_err(to_ui_error)?;
    let store = checkpoint_store(&app)?;
    let progress = store
        .load_progress()
        .map_err(to_ui_error)?
        .ok_or(RestoreError::MissingScan)
        .map_err(to_ui_error)?;

    if progress.failed_ids.is_empty() {
        return Err(encode_message(&msg("status.retryFailedEmpty")));
    }

    run_restore(app, window, state, credentials, progress.failed_ids).await
}

#[tauri::command]
pub async fn pause_restore(state: State<'_, AppState>) -> Result<UiSnapshot, String> {
    {
        let session = state.session.lock().await;
        if let Some(token) = &session.cancellation {
            token.cancel();
        }
    }
    update_phase(&state, AppPhase::Paused, msg("status.pausePending")).await;
    get_restore_state(state).await
}

async fn run_restore(
    app: AppHandle,
    window: Window,
    state: State<'_, AppState>,
    credentials: Credentials,
    item_ids: Vec<String>,
) -> Result<UiSnapshot, String> {
    let store = checkpoint_store(&app)?;
    let api = ICloudApiClient::with_base_url(credentials.resolved_docws_base_url())
        .map_err(to_ui_error)?;
    let supervisor = RestoreSupervisor::new(api, store);
    let cancellation = CancellationToken::default();

    {
        let mut session = state.session.lock().await;
        session.phase = AppPhase::Restoring;
        session.cancellation = Some(cancellation.clone());
        session.message = msg("status.restoring");
    }

    let result = supervisor
        .restore_items(credentials, item_ids, cancellation, |event| {
            let _ = emit_event(&window, event);
        })
        .await;

    {
        let mut session = state.session.lock().await;
        session.cancellation = None;
        match result {
            Ok(stats) => {
                session.phase = AppPhase::Complete;
                session.message = if stats.failed == 0 {
                    msg("status.complete").with_param("restored", stats.restored)
                } else {
                    msg("status.partialComplete")
                        .with_param("restored", stats.restored)
                        .with_param("failed", stats.failed)
                };
                session.stats = stats;
            }
            Err(RestoreError::Cancelled) => {
                session.phase = AppPhase::Paused;
                session.message = msg("status.paused");
            }
            Err(error) => {
                session.phase = AppPhase::Error;
                session.message = error.message();
                return Err(encode_message(&session.message));
            }
        }
    }

    get_restore_state(state).await
}

async fn credentials(state: &State<'_, AppState>) -> Result<Credentials, RestoreError> {
    state
        .session
        .lock()
        .await
        .credentials
        .clone()
        .ok_or(RestoreError::MissingCredentials)
}

async fn scanned_item_ids(state: &State<'_, AppState>) -> Result<Vec<String>, RestoreError> {
    let ids = state.session.lock().await.deleted_item_ids.clone();
    if ids.is_empty() {
        Err(RestoreError::MissingScan)
    } else {
        Ok(ids)
    }
}

async fn update_phase(state: &State<'_, AppState>, phase: AppPhase, message: UiMessage) {
    let mut session = state.session.lock().await;
    session.phase = phase;
    session.message = message;
}

fn checkpoint_store(app: &AppHandle) -> Result<CheckpointStore, String> {
    let dir: PathBuf = app
        .path()
        .app_data_dir()
        .map_err(|e| encode_message(&msg("error.appDataDir").with_param("details", e)))?;
    CheckpointStore::new(dir).map_err(to_ui_error)
}

fn emit_event(window: &Window, event: RestoreEvent) -> Result<(), String> {
    window
        .emit("restore-event", event)
        .map_err(|e| encode_message(&msg("error.windowUpdate").with_param("details", e)))
}

fn to_ui_error(error: RestoreError) -> String {
    let message = error.message();
    encode_message(&UiMessage {
        id: message.id,
        params: message
            .params
            .into_iter()
            .map(|(key, value)| (key, redact_sensitive(&value)))
            .collect(),
    })
}

fn msg(id: &str) -> UiMessage {
    UiMessage::new(id)
}

fn encode_message(message: &UiMessage) -> String {
    serde_json::to_string(message)
        .unwrap_or_else(|_| "{\"id\":\"error.unknown\",\"params\":{}}".to_string())
}

fn redact_sensitive(value: &str) -> String {
    let mut redacted = value.to_string();
    for key in [
        "clientId=",
        "dsid=",
        "clientBuildNumber=",
        "clientMasteringNumber=",
        "Cookie:",
        "cookie:",
    ] {
        redacted = redact_after_key(&redacted, key);
    }
    redacted
}

fn redact_after_key(value: &str, key: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;

    while let Some(index) = remaining.find(key) {
        let (before, after_before) = remaining.split_at(index);
        output.push_str(before);
        output.push_str(key);

        let secret_start = key.len();
        let mut secret = &after_before[secret_start..];
        let whitespace_len = secret
            .chars()
            .take_while(|character| character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        output.push_str(&secret[..whitespace_len]);
        output.push_str("[redacted]");
        secret = &secret[whitespace_len..];
        let secret_end = secret
            .find(['&', ' ', '"', '\'', ')', ','])
            .unwrap_or(secret.len());
        remaining = &secret[secret_end..];
    }

    output.push_str(remaining);
    output
}

impl AppSession {
    fn snapshot(&self, can_resume: bool) -> UiSnapshot {
        let deleted_count = if self.deleted_item_ids.is_empty() {
            self.partial_scan_item_count
        } else {
            self.deleted_item_ids.len()
        };
        UiSnapshot {
            phase: self.phase.clone(),
            deleted_count,
            stats: self.stats.clone(),
            message: self.message.clone(),
            can_resume,
        }
    }
}

impl Default for AppSession {
    fn default() -> Self {
        Self {
            phase: AppPhase::Welcome,
            credentials: None,
            browser: None,
            deleted_item_ids: Vec::new(),
            partial_scan_item_count: 0,
            stats: RestoreStats::default(),
            message: msg("status.ready"),
            cancellation: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_errors_redact_sensitive_query_values() {
        let error =
            "request failed for https://www.icloud.com/?clientId=abc&dsid=123 cookie: token";

        assert_eq!(
            redact_sensitive(error),
            "request failed for https://www.icloud.com/?clientId=[redacted]&dsid=[redacted] cookie: [redacted]"
        );
    }
}
