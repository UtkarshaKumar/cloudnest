use crate::restore::models::{
    Credentials, RestoreError, CHROME_DEBUG_URL, DEFAULT_CLIENT_BUILD, ICLOUD_RECOVERY_URL,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use url::Url;

#[derive(Debug)]
pub struct ChromeAuthenticator {
    client: reqwest::Client,
    temp_profile: Option<TempDir>,
    chrome_child: Option<Child>,
}

#[derive(Debug, Clone, Deserialize)]
struct CdpTarget {
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_debugger_url: Option<String>,
    url: Option<String>,
    #[serde(rename = "type")]
    target_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRequestCredentials {
    pub client_id: String,
    pub dsid: String,
    pub client_build_number: String,
    pub client_mastering_number: String,
}

impl ChromeAuthenticator {
    pub fn new() -> Result<Self, RestoreError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| RestoreError::Network(e.to_string()))?;

        Ok(Self {
            client,
            temp_profile: None,
            chrome_child: None,
        })
    }

    pub async fn connect_or_launch(&mut self) -> Result<(), RestoreError> {
        if is_port_open(9222) {
            return Ok(());
        }

        let chrome_path = chrome_path().ok_or(RestoreError::ChromeMissing)?;
        let temp_profile = tempfile::Builder::new()
            .prefix("cloudnest-")
            .tempdir()
            .map_err(|e| RestoreError::File(e.to_string()))?;

        let child = Command::new(chrome_path)
            .arg("--remote-debugging-port=9222")
            .arg(format!("--user-data-dir={}", temp_profile.path().display()))
            .arg("--no-first-run")
            .arg(ICLOUD_RECOVERY_URL)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| RestoreError::ChromeLaunchFailed)?;

        self.temp_profile = Some(temp_profile);
        self.chrome_child = Some(child);

        for attempt in 0..8 {
            sleep(Duration::from_millis(500 + attempt * 250)).await;
            if is_port_open(9222) {
                return Ok(());
            }
        }

        Err(RestoreError::ChromeConnectionFailed)
    }

    pub async fn wait_for_login(&self, timeout_seconds: u64) -> Result<Credentials, RestoreError> {
        let login = timeout(
            Duration::from_secs(timeout_seconds),
            self.wait_for_login_inner(),
        )
        .await
        .map_err(|_| RestoreError::LoginTimeout)??;

        Ok(login)
    }

    pub async fn refresh_credentials(&self) -> Result<Credentials, RestoreError> {
        self.wait_for_login(300).await
    }

    async fn wait_for_login_inner(&self) -> Result<Credentials, RestoreError> {
        let target = self.pick_page_target().await?;
        let websocket_url = target
            .websocket_debugger_url
            .ok_or(RestoreError::ChromeConnectionFailed)?;
        let (mut ws, _) = connect_async(&websocket_url)
            .await
            .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?;

        send_cdp(&mut ws, 1, "Network.enable", json!({})).await?;
        send_cdp(&mut ws, 2, "Page.enable", json!({})).await?;
        if !target.url.as_deref().unwrap_or_default().contains("icloud.com") {
            send_cdp(
                &mut ws,
                3,
                "Page.navigate",
                json!({ "url": ICLOUD_RECOVERY_URL }),
            )
            .await?;
        } else {
            send_cdp(&mut ws, 3, "Page.reload", json!({})).await?;
        }

        let mut captured: Option<CapturedRequestCredentials> = None;
        let mut cookie_request_id: Option<u64> = None;

        while let Some(message) = ws.next().await {
            let message = message.map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?;
            if !message.is_text() {
                continue;
            }

            let value: Value = serde_json::from_str(message.to_text().unwrap_or(""))
                .map_err(|e| RestoreError::Api(e.to_string()))?;

            if captured.is_none() {
                if let Some(url) = value
                    .pointer("/params/request/url")
                    .and_then(Value::as_str)
                {
                    captured = capture_credentials_from_url(url);
                    if captured.is_some() {
                        cookie_request_id = Some(4);
                        send_cdp(
                            &mut ws,
                            4,
                            "Network.getCookies",
                            json!({ "urls": ["https://www.icloud.com"] }),
                        )
                        .await?;
                    }
                }
            }

            if value.get("id").and_then(Value::as_u64) == cookie_request_id {
                let cookies = parse_cdp_cookies(&value)?;
                if let Some(captured) = captured {
                    return Ok(Credentials {
                        cookies,
                        client_id: captured.client_id,
                        dsid: captured.dsid,
                        client_build_number: captured.client_build_number,
                        client_mastering_number: captured.client_mastering_number,
                    });
                }
            }
        }

        Err(RestoreError::LoginTimeout)
    }

    async fn pick_page_target(&self) -> Result<CdpTarget, RestoreError> {
        let targets: Vec<CdpTarget> = self
            .client
            .get(format!("{CHROME_DEBUG_URL}/json"))
            .send()
            .await
            .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?
            .json()
            .await
            .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?;

        targets
            .iter()
            .cloned()
            .find(|target| {
                target.target_type.as_deref() == Some("page")
                    && target.websocket_debugger_url.is_some()
                    && target
                        .url
                        .as_deref()
                        .is_some_and(|url| url.contains("icloud.com"))
            })
            .or_else(|| {
                targets.into_iter().find(|target| {
                    target.target_type.as_deref() == Some("page")
                        && target.websocket_debugger_url.is_some()
                })
            })
            .ok_or(RestoreError::ChromeConnectionFailed)
    }
}

trait RestoreErrorDetail {
    fn with_detail(self, detail: String) -> RestoreError;
}

impl RestoreErrorDetail for RestoreError {
    fn with_detail(self, detail: String) -> RestoreError {
        match self {
            RestoreError::ChromeConnectionFailed => {
                RestoreError::Api(format!("Chrome connection failed: {detail}"))
            }
            other => other,
        }
    }
}

fn chrome_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let path = PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        if path.exists() {
            return Some(path);
        }
    }

    #[cfg(target_os = "windows")]
    {
        for path in [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ] {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for path in [
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ] {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }
    }

    None
}

fn is_port_open(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

async fn send_cdp<S>(
    ws: &mut S,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), RestoreError>
where
    S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
    <S as futures_util::Sink<tokio_tungstenite::tungstenite::Message>>::Error: std::fmt::Display,
{
    let message = json!({
        "id": id,
        "method": method,
        "params": params,
    });
    ws.send(tokio_tungstenite::tungstenite::Message::Text(message.to_string().into()))
        .await
        .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))
}

pub fn capture_credentials_from_url(url: &str) -> Option<CapturedRequestCredentials> {
    if !url.contains("icloud.com") {
        return None;
    }

    let parsed = Url::parse(url).ok()?;
    let mut client_id = None;
    let mut dsid = None;
    let mut client_build_number = None;
    let mut client_mastering_number = None;

    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "clientId" => client_id = Some(value.to_string()),
            "dsid" => dsid = Some(value.to_string()),
            "clientBuildNumber" => client_build_number = Some(value.to_string()),
            "clientMasteringNumber" => client_mastering_number = Some(value.to_string()),
            _ => {}
        }
    }

    Some(CapturedRequestCredentials {
        client_id: client_id?,
        dsid: dsid?,
        client_build_number: client_build_number.unwrap_or_else(|| DEFAULT_CLIENT_BUILD.to_string()),
        client_mastering_number: client_mastering_number
            .unwrap_or_else(|| DEFAULT_CLIENT_BUILD.to_string()),
    })
}

fn parse_cdp_cookies(value: &Value) -> Result<String, RestoreError> {
    let cookies = value
        .pointer("/result/cookies")
        .and_then(Value::as_array)
        .ok_or_else(|| RestoreError::Api("Chrome did not return iCloud cookies.".to_string()))?;

    Ok(cookies
        .iter()
        .filter_map(|cookie| {
            let name = cookie.get("name")?.as_str()?;
            let value = cookie.get("value")?.as_str()?;
            Some(format!("{name}={value}"))
        })
        .collect::<Vec<_>>()
        .join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_credentials_from_icloud_url() {
        let url = "https://www.icloud.com/recovery/?clientId=abc&dsid=123&clientBuildNumber=b1&clientMasteringNumber=m1";

        let captured = capture_credentials_from_url(url).unwrap();

        assert_eq!(captured.client_id, "abc");
        assert_eq!(captured.dsid, "123");
        assert_eq!(captured.client_build_number, "b1");
        assert_eq!(captured.client_mastering_number, "m1");
    }

    #[test]
    fn ignores_urls_without_credentials() {
        assert!(capture_credentials_from_url("https://www.icloud.com/recovery/").is_none());
        assert!(capture_credentials_from_url("https://example.com/?clientId=a&dsid=b").is_none());
    }

    #[test]
    fn parses_cdp_cookies_into_header() {
        let value = json!({
            "id": 4,
            "result": {
                "cookies": [
                    {"name": "a", "value": "1"},
                    {"name": "b", "value": "2"}
                ]
            }
        });

        assert_eq!(parse_cdp_cookies(&value).unwrap(), "a=1; b=2");
    }
}
