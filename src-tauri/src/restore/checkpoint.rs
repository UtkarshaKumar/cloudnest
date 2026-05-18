use crate::restore::models::RestoreError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FetchCheckpoint {
    pub item_ids: Vec<String>,
    pub continuation_marker: Option<String>,
    pub page: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreProgress {
    pub restored_ids: Vec<String>,
    pub failed_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CheckpointStore {
    dir: PathBuf,
}

impl CheckpointStore {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, RestoreError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| RestoreError::File(e.to_string()))?;
        Ok(Self { dir })
    }

    pub fn checkpoint_path(&self) -> PathBuf {
        self.dir.join("icloud_restore_checkpoint.json")
    }

    pub fn progress_path(&self) -> PathBuf {
        self.dir.join("icloud_restore_progress.json")
    }

    pub fn load_checkpoint(&self) -> Result<Option<FetchCheckpoint>, RestoreError> {
        read_json(&self.checkpoint_path())
    }

    pub fn save_checkpoint(&self, checkpoint: &FetchCheckpoint) -> Result<(), RestoreError> {
        write_json_atomic(&self.checkpoint_path(), checkpoint)
    }

    pub fn load_progress(&self) -> Result<Option<RestoreProgress>, RestoreError> {
        read_json(&self.progress_path())
    }

    pub fn save_progress(&self, progress: &RestoreProgress) -> Result<(), RestoreError> {
        write_json_atomic(&self.progress_path(), progress)
    }

    pub fn clear_checkpoint(&self) -> Result<(), RestoreError> {
        remove_if_exists(&self.checkpoint_path())
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, RestoreError> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).map_err(|e| RestoreError::File(e.to_string()))?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|e| RestoreError::ProgressCorrupt(e.to_string()))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), RestoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| RestoreError::File("Progress path has no parent directory".to_string()))?;
    fs::create_dir_all(parent).map_err(|e| RestoreError::File(e.to_string()))?;
    let temp_path = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(value).map_err(|e| RestoreError::File(e.to_string()))?;

    fs::write(&temp_path, body).map_err(|e| RestoreError::File(e.to_string()))?;
    fs::rename(&temp_path, path).map_err(|e| RestoreError::File(e.to_string()))?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), RestoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RestoreError::File(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(temp.path()).unwrap();
        let checkpoint = FetchCheckpoint {
            item_ids: vec!["a".to_string(), "b".to_string()],
            continuation_marker: Some("next".to_string()),
            page: 2,
            complete: false,
        };

        store.save_checkpoint(&checkpoint).unwrap();

        assert_eq!(store.load_checkpoint().unwrap(), Some(checkpoint));
    }

    #[test]
    fn corrupt_progress_is_not_silently_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(temp.path()).unwrap();
        fs::write(store.progress_path(), "{not-json").unwrap();

        let error = store.load_progress().unwrap_err();

        assert!(matches!(error, RestoreError::ProgressCorrupt(_)));
    }

    #[test]
    fn progress_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(temp.path()).unwrap();
        let progress = RestoreProgress {
            restored_ids: vec!["ok-1".to_string()],
            failed_ids: vec!["bad-1".to_string()],
        };

        store.save_progress(&progress).unwrap();

        assert_eq!(store.load_progress().unwrap(), Some(progress));
    }
}
