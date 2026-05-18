use crate::restore::api::{default_restore_batches, max_retries, ICloudApiClient};
use crate::restore::checkpoint::{CheckpointStore, RestoreProgress};
use crate::restore::models::{Credentials, RestoreError, RestoreEvent, RestoreStats};
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::time::{sleep, Duration};

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

#[derive(Debug, Clone)]
pub struct RestoreSupervisor {
    api: ICloudApiClient,
    store: CheckpointStore,
}

impl RestoreSupervisor {
    pub fn new(api: ICloudApiClient, store: CheckpointStore) -> Self {
        Self { api, store }
    }

    pub async fn restore_items<F>(
        &self,
        credentials: Credentials,
        item_ids: Vec<String>,
        cancellation: CancellationToken,
        mut emit: F,
    ) -> Result<RestoreStats, RestoreError>
    where
        F: FnMut(RestoreEvent),
    {
        let mut progress = self.store.load_progress()?.unwrap_or_default();
        dedupe(&mut progress.restored_ids);
        dedupe(&mut progress.failed_ids);

        let restored: HashSet<String> = progress.restored_ids.iter().cloned().collect();
        let remaining_ids: Vec<String> = item_ids
            .into_iter()
            .filter(|item_id| !restored.contains(item_id))
            .collect();
        let total = restored.len() + remaining_ids.len();

        if remaining_ids.is_empty() {
            let stats = RestoreStats {
                total,
                restored: restored.len(),
                failed: progress.failed_ids.len(),
                failed_ids: progress.failed_ids,
            };
            emit(RestoreEvent::Complete {
                stats: stats.clone(),
            });
            return Ok(stats);
        }

        emit(RestoreEvent::RestoreStarted { total });
        let batches = default_restore_batches(&remaining_ids);
        let started = Instant::now();
        let mut restored_count = restored.len();
        let mut failed_count = 0usize;

        for (batch_index, batch) in batches.iter().enumerate() {
            if cancellation.is_cancelled() {
                self.store.save_progress(&progress)?;
                emit(RestoreEvent::Paused {
                    message: "Restore paused after the current batch. Progress is saved.".to_string(),
                });
                return Err(RestoreError::Cancelled);
            }

            let mut restored_batch = false;
            for attempt in 0..max_retries() {
                match self.api.restore_batch(&credentials, batch).await {
                    Ok(()) => {
                        restored_batch = true;
                        restored_count += batch.len();
                        progress.restored_ids.extend(batch.iter().cloned());
                        dedupe(&mut progress.restored_ids);
                        self.store.save_progress(&progress)?;
                        emit(RestoreEvent::RestoreProgress {
                            total,
                            restored: restored_count,
                            failed: failed_count,
                            eta_seconds: estimate_eta_seconds(started, restored_count + failed_count, total),
                            message: format!(
                                "Batch {}/{} restored.",
                                batch_index + 1,
                                batches.len()
                            ),
                        });
                        break;
                    }
                    Err(RestoreError::AuthExpired) => return Err(RestoreError::AuthExpired),
                    Err(error) if attempt + 1 < max_retries() => {
                        let delay = Duration::from_secs(2u64.saturating_pow(attempt as u32));
                        emit(RestoreEvent::Retry {
                            batch_number: batch_index + 1,
                            attempt: attempt + 1,
                            message: format!(
                                "iCloud is taking longer than usual. Retrying this batch automatically. ({error})"
                            ),
                        });
                        sleep(delay).await;
                    }
                    Err(_) => break,
                }
            }

            if !restored_batch {
                failed_count += batch.len();
                progress.failed_ids.extend(batch.iter().cloned());
                dedupe(&mut progress.failed_ids);
                self.store.save_progress(&progress)?;
                emit(RestoreEvent::RestoreProgress {
                    total,
                    restored: restored_count,
                    failed: failed_count,
                    eta_seconds: estimate_eta_seconds(started, restored_count + failed_count, total),
                    message: format!(
                        "Batch {}/{} needs another try.",
                        batch_index + 1,
                        batches.len()
                    ),
                });
            }
        }

        let stats = RestoreStats {
            total,
            restored: progress.restored_ids.len(),
            failed: progress.failed_ids.len(),
            failed_ids: progress.failed_ids.clone(),
        };
        emit(RestoreEvent::Complete {
            stats: stats.clone(),
        });
        Ok(stats)
    }
}

pub fn remaining_item_ids(item_ids: &[String], progress: &RestoreProgress) -> Vec<String> {
    let restored: HashSet<&String> = progress.restored_ids.iter().collect();
    item_ids
        .iter()
        .filter(|item_id| !restored.contains(item_id))
        .cloned()
        .collect()
}

pub fn merge_failed_for_retry(progress: &RestoreProgress) -> Vec<String> {
    let mut ids = progress.failed_ids.clone();
    dedupe(&mut ids);
    ids
}

fn dedupe(ids: &mut Vec<String>) {
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
}

fn estimate_eta_seconds(started: Instant, completed: usize, total: usize) -> Option<u64> {
    if completed == 0 || completed >= total {
        return None;
    }

    let elapsed = started.elapsed().as_secs_f64();
    if elapsed <= 0.0 {
        return None;
    }

    let rate = completed as f64 / elapsed;
    if rate <= 0.0 {
        return None;
    }

    Some(((total - completed) as f64 / rate).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_item_ids_skips_already_restored_items() {
        let item_ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let progress = RestoreProgress {
            restored_ids: vec!["b".to_string()],
            failed_ids: Vec::new(),
        };

        assert_eq!(remaining_item_ids(&item_ids, &progress), vec!["a", "c"]);
    }

    #[test]
    fn retry_failed_ids_are_deduped() {
        let progress = RestoreProgress {
            restored_ids: Vec::new(),
            failed_ids: vec!["a".to_string(), "a".to_string(), "b".to_string()],
        };

        assert_eq!(merge_failed_for_retry(&progress), vec!["a", "b"]);
    }

    #[test]
    fn cancellation_token_tracks_cancel_state() {
        let token = CancellationToken::default();
        assert!(!token.is_cancelled());

        token.cancel();

        assert!(token.is_cancelled());
    }
}
