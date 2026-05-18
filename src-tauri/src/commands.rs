use crate::restore::api::ICloudApiClient;
use crate::restore::browser::ChromeAuthenticator;
use crate::restore::checkpoint::CheckpointStore;
use crate::restore::job::{CancellationToken, RestoreSupervisor};
use crate::restore::models::{
    AppPhase, Credentials, RestoreError, RestoreEvent, RestoreStats, UiSnapshot,
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
    stats: RestoreStats,
    message: String,
    cancellation: Option<CancellationToken>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(AppSession {
                phase: AppPhase::Welcome,
                credentials: None,
                browser: None,
                deleted_item_ids: Vec::new(),
                stats: RestoreStats::default(),
                message: "Ready to recover deleted iCloud Drive files.".to_string(),
                cancellation: None,
            }),
        }
    }
}

#[tauri::command]
pub async fn get_restore_state(state: State<'_, AppState>) -> Result<UiSnapshot, String> {
    let session = state.session.lock().await;
    Ok(session.snapshot(false))
}

#[tauri::command]
pub async fn start_auth(window: Window, state: State<'_, AppState>) -> Result<UiSnapshot, String> {
    emit_event(&window, RestoreEvent::AuthStarted)?;
    update_phase(
        &state,
        AppPhase::SigningIn,
        "Waiting for iCloud sign-in...".to_string(),
    )
    .await;

    let mut browser = ChromeAuthenticator::new().map_err(to_ui_error)?;
    browser.connect_or_launch().await.map_err(to_ui_error)?;
    let credentials = browser.wait_for_login(300).await.map_err(to_ui_error)?;

    {
        let mut session = state.session.lock().await;
        session.credentials = Some(credentials);
        session.browser = Some(browser);
        session.phase = AppPhase::ReadyToScan;
        session.message = "Sign-in detected. Ready to scan deleted items.".to_string();
    }

    emit_event(&window, RestoreEvent::Authenticated)?;
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
    let api = ICloudApiClient::new().map_err(to_ui_error)?;

    update_phase(
        &state,
        AppPhase::Scanning,
        "Finding deleted files and folders...".to_string(),
    )
    .await;

    let items = api
        .fetch_deleted_items(&credentials, Some(&store), |progress| {
            let _ = emit_event(
                &window,
                RestoreEvent::ScanProgress {
                    page: progress.page,
                    page_count: progress.page_count,
                    total: progress.total,
                },
            );
        })
        .await
        .map_err(to_ui_error)?;

    let item_ids: Vec<String> = items.into_iter().map(|item| item.item_id).collect();
    {
        let mut session = state.session.lock().await;
        session.deleted_item_ids = item_ids;
        session.phase = AppPhase::ReadyToRestore;
        session.message = format!(
            "{} deleted iCloud Drive items are ready to restore.",
            session.deleted_item_ids.len()
        );
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
        return Err("There are no failed items to retry.".to_string());
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
    update_phase(
        &state,
        AppPhase::Paused,
        "Restore will pause after the current batch. Progress is saved.".to_string(),
    )
    .await;
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
    let api = ICloudApiClient::new().map_err(to_ui_error)?;
    let supervisor = RestoreSupervisor::new(api, store);
    let cancellation = CancellationToken::default();

    {
        let mut session = state.session.lock().await;
        session.phase = AppPhase::Restoring;
        session.cancellation = Some(cancellation.clone());
        session.message = "Restoring your files...".to_string();
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
                    format!(
                        "Recovery complete. {} items were restored to iCloud Drive.",
                        stats.restored
                    )
                } else {
                    format!(
                        "Restored {} items. {} items need another try.",
                        stats.restored, stats.failed
                    )
                };
                session.stats = stats;
            }
            Err(RestoreError::Cancelled) => {
                session.phase = AppPhase::Paused;
                session.message = "Restore paused. Progress is saved.".to_string();
            }
            Err(error) => {
                session.phase = AppPhase::Error;
                session.message = error.to_string();
                return Err(session.message.clone());
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

async fn update_phase(state: &State<'_, AppState>, phase: AppPhase, message: String) {
    let mut session = state.session.lock().await;
    session.phase = phase;
    session.message = message;
}

fn checkpoint_store(app: &AppHandle) -> Result<CheckpointStore, String> {
    let dir: PathBuf = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not open app data folder: {e}"))?;
    CheckpointStore::new(dir).map_err(to_ui_error)
}

fn emit_event(window: &Window, event: RestoreEvent) -> Result<(), String> {
    window
        .emit("restore-event", event)
        .map_err(|e| format!("Could not update the app window: {e}"))
}

fn to_ui_error(error: RestoreError) -> String {
    error.to_string()
}

impl AppSession {
    fn snapshot(&self, can_resume: bool) -> UiSnapshot {
        UiSnapshot {
            phase: self.phase.clone(),
            deleted_count: self.deleted_item_ids.len(),
            stats: self.stats.clone(),
            message: self.message.clone(),
            can_resume,
        }
    }
}
