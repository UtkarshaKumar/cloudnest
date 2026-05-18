use crate::restore::checkpoint::CheckpointStore;
use crate::restore::models::{
    Credentials, DeletedItem, RestoreError, DEFAULT_FETCH_PAGE_SIZE, DEFAULT_MAX_RETRIES,
    DEFAULT_RESTORE_BATCH_SIZE,
};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, COOKIE, ORIGIN, REFERER};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use url::Url;

pub const DEFAULT_BASE_URL: &str = "https://p107-docws.icloud.com";

#[derive(Debug, Clone)]
pub struct ICloudApiClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchProgress {
    pub page: u64,
    pub page_count: usize,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
struct TombstoneResponse {
    #[serde(default)]
    documents: Vec<TombstoneDocument>,
    #[serde(rename = "continuationMarker")]
    continuation_marker: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TombstoneDocument {
    item_id: Option<String>,
    name: Option<String>,
    #[serde(rename = "type")]
    item_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RestoreResponse {
    #[serde(default)]
    drive_items_with_status: Vec<ItemStatus>,
}

#[derive(Debug, Deserialize)]
struct ItemStatus {
    status_code: Option<serde_json::Value>,
    status_message: Option<String>,
}

impl ICloudApiClient {
    pub fn new() -> Result<Self, RestoreError> {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self, RestoreError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| RestoreError::Network(e.to_string()))?;

        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    pub async fn fetch_deleted_items<F>(
        &self,
        credentials: &Credentials,
        checkpoint_store: Option<&CheckpointStore>,
        mut on_progress: F,
    ) -> Result<Vec<DeletedItem>, RestoreError>
    where
        F: FnMut(FetchProgress),
    {
        let mut checkpoint = checkpoint_store
            .map(CheckpointStore::load_checkpoint)
            .transpose()?
            .flatten()
            .unwrap_or_default();

        if checkpoint.complete && !checkpoint.item_ids.is_empty() {
            on_progress(FetchProgress {
                page: checkpoint.page,
                page_count: 0,
                total: checkpoint.item_ids.len(),
            });
            return Ok(checkpoint
                .item_ids
                .iter()
                .map(|item_id| DeletedItem {
                    item_id: item_id.clone(),
                    name: None,
                    item_type: None,
                })
                .collect());
        }

        let mut all_items: Vec<DeletedItem> = checkpoint
            .item_ids
            .iter()
            .map(|item_id| DeletedItem {
                item_id: item_id.clone(),
                name: None,
                item_type: None,
            })
            .collect();

        loop {
            checkpoint.page += 1;
            let url = self.tombstones_url(credentials, checkpoint.continuation_marker.as_deref())?;
            let response = self
                .client
                .get(url)
                .headers(headers(credentials)?)
                .send()
                .await?;

            ensure_auth(response.status().as_u16())?;
            let response = response
                .error_for_status()
                .map_err(|e| RestoreError::Api(e.to_string()))?;
            let data: TombstoneResponse = response.json().await?;
            let page_items = parse_tombstones(data.documents);

            checkpoint.item_ids.extend(page_items.iter().map(|item| item.item_id.clone()));
            all_items.extend(page_items.iter().cloned());
            checkpoint.continuation_marker = data.continuation_marker;
            checkpoint.complete = data.status.as_deref() != Some("MORE_AVAILABLE")
                || checkpoint.continuation_marker.is_none();

            if let Some(store) = checkpoint_store {
                store.save_checkpoint(&checkpoint)?;
            }

            on_progress(FetchProgress {
                page: checkpoint.page,
                page_count: page_items.len(),
                total: all_items.len(),
            });

            if checkpoint.complete {
                break;
            }
        }

        Ok(all_items)
    }

    pub async fn restore_batch(
        &self,
        credentials: &Credentials,
        batch: &[String],
    ) -> Result<(), RestoreError> {
        let url = self.restore_url(credentials)?;
        let body = json!({
            "drive_item_update_request": { "is_recover": "true" },
            "item_ids": batch,
        });

        let response = self
            .client
            .put(url)
            .headers(headers(credentials)?)
            .json(&body)
            .send()
            .await?;

        ensure_auth(response.status().as_u16())?;
        let response = response
            .error_for_status()
            .map_err(|e| RestoreError::Api(e.to_string()))?;
        let data: RestoreResponse = response.json().await?;

        if let Some(status) = data.drive_items_with_status.first() {
            let status_code = status_code_as_string(status.status_code.as_ref());
            if status_code.as_deref().is_some_and(|code| code != "200") {
                let message = status
                    .status_message
                    .clone()
                    .unwrap_or_else(|| "iCloud did not restore this batch.".to_string());
                return Err(RestoreError::Api(message));
            }
        }

        Ok(())
    }

    fn tombstones_url(
        &self,
        credentials: &Credentials,
        continuation_marker: Option<&str>,
    ) -> Result<Url, RestoreError> {
        let mut url = Url::parse(&format!(
            "{}/ws/_all_/list/enumerate/tombstones",
            self.base_url.trim_end_matches('/')
        ))
        .map_err(|e| RestoreError::Api(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("clientBuildNumber", &credentials.client_build_number);
            query.append_pair("clientMasteringNumber", &credentials.client_mastering_number);
            query.append_pair("clientId", &credentials.client_id);
            query.append_pair("dsid", &credentials.dsid);
            query.append_pair("limit", &DEFAULT_FETCH_PAGE_SIZE.to_string());
            query.append_pair("unified_format", "true");
            if let Some(marker) = continuation_marker {
                query.append_pair("nextPage", marker);
            }
        }
        Ok(url)
    }

    fn restore_url(&self, credentials: &Credentials) -> Result<Url, RestoreError> {
        let mut url = Url::parse(&format!("{}/v1/items", self.base_url.trim_end_matches('/')))
            .map_err(|e| RestoreError::Api(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("clientBuildNumber", &credentials.client_build_number);
            query.append_pair("clientMasteringNumber", &credentials.client_mastering_number);
            query.append_pair("clientId", &credentials.client_id);
            query.append_pair("dsid", &credentials.dsid);
        }
        Ok(url)
    }
}

pub fn batch_item_ids(item_ids: &[String], batch_size: usize) -> Vec<Vec<String>> {
    if batch_size == 0 {
        return Vec::new();
    }

    item_ids
        .chunks(batch_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

pub fn default_restore_batches(item_ids: &[String]) -> Vec<Vec<String>> {
    batch_item_ids(item_ids, DEFAULT_RESTORE_BATCH_SIZE)
}

pub fn max_retries() -> usize {
    DEFAULT_MAX_RETRIES
}

fn headers(credentials: &Credentials) -> Result<HeaderMap, RestoreError> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    headers.insert(ORIGIN, HeaderValue::from_static("https://www.icloud.com"));
    headers.insert(REFERER, HeaderValue::from_static("https://www.icloud.com/"));
    headers.insert(
        COOKIE,
        HeaderValue::from_str(&credentials.cookies)
            .map_err(|e| RestoreError::Api(format!("Invalid iCloud cookie header: {e}")))?,
    );
    Ok(headers)
}

fn parse_tombstones(documents: Vec<TombstoneDocument>) -> Vec<DeletedItem> {
    documents
        .into_iter()
        .filter_map(|doc| {
            doc.item_id.map(|item_id| DeletedItem {
                item_id,
                name: doc.name,
                item_type: doc.item_type,
            })
        })
        .collect()
}

fn ensure_auth(status: u16) -> Result<(), RestoreError> {
    if matches!(status, 401 | 403 | 421) {
        Err(RestoreError::AuthExpired)
    } else {
        Ok(())
    }
}

fn status_code_as_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(serde_json::Value::Number(value)) => Some(value.to_string()),
        Some(value) => Some(value.to_string()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn creds() -> Credentials {
        Credentials::new("ck=value".to_string(), "client".to_string(), "dsid".to_string())
    }

    #[test]
    fn batches_item_ids_in_requested_size() {
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];

        assert_eq!(
            batch_item_ids(&ids, 2),
            vec![vec!["a".to_string(), "b".to_string()], vec!["c".to_string()]]
        );
    }

    #[test]
    fn zero_batch_size_returns_no_batches() {
        let ids = vec!["a".to_string()];

        assert!(batch_item_ids(&ids, 0).is_empty());
    }

    #[tokio::test]
    async fn fetch_deleted_items_resumes_from_checkpoint_marker() {
        let server = MockServer::start();
        let temp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(temp.path()).unwrap();
        store
            .save_checkpoint(&crate::restore::checkpoint::FetchCheckpoint {
                item_ids: vec!["one".to_string()],
                continuation_marker: Some("next".to_string()),
                page: 1,
                complete: false,
            })
            .unwrap();
        let next_page = server.mock(|when, then| {
            when.method(GET)
                .path("/ws/_all_/list/enumerate/tombstones")
                .query_param("nextPage", "next");
            then.status(200).json_body(json!({
                "status": "OK",
                "documents": [{"item_id": "two", "type": "FILE"}]
            }));
        });
        let client = ICloudApiClient::with_base_url(server.base_url()).unwrap();

        let items = client
            .fetch_deleted_items(&creds(), Some(&store), |_| {})
            .await
            .unwrap();

        next_page.assert();
        assert_eq!(items.iter().map(|item| item.item_id.as_str()).collect::<Vec<_>>(), vec!["one", "two"]);
    }

    #[tokio::test]
    async fn restore_batch_maps_auth_status_to_auth_expired() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v1/items");
            then.status(421).body("expired");
        });
        let client = ICloudApiClient::with_base_url(server.base_url()).unwrap();

        let error = client
            .restore_batch(&creds(), &["one".to_string()])
            .await
            .unwrap_err();

        assert!(matches!(error, RestoreError::AuthExpired));
    }

    #[tokio::test]
    async fn restore_batch_detects_item_status_failure() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v1/items");
            then.status(200).json_body(json!({
                "drive_items_with_status": [{"status_code": "503", "status_message": "try later"}]
            }));
        });
        let client = ICloudApiClient::with_base_url(server.base_url()).unwrap();

        let error = client
            .restore_batch(&creds(), &["one".to_string()])
            .await
            .unwrap_err();

        assert!(matches!(error, RestoreError::Api(message) if message == "try later"));
    }
}
