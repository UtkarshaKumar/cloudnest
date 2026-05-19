use crate::restore::models::{
    Credentials, RestoreError, DEFAULT_CLIENT_BUILD, ICLOUD_RECOVERY_URL,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use url::Url;

#[derive(Debug)]
pub struct ChromeAuthenticator {
    client: reqwest::Client,
    debug_port: u16,
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
            debug_port: allocate_debug_port()?,
            temp_profile: None,
            chrome_child: None,
        })
    }

    pub async fn connect_or_launch(&mut self) -> Result<(), RestoreError> {
        if self.chrome_child.is_some() && is_port_open(self.debug_port) {
            return Ok(());
        }
        self.shutdown().await;

        let chrome_path = chrome_path().ok_or(RestoreError::ChromeMissing)?;
        let temp_profile = tempfile::Builder::new()
            .prefix("cloudnest-")
            .tempdir()
            .map_err(|e| RestoreError::File(e.to_string()))?;

        let child = Command::new(chrome_path)
            .arg(format!("--remote-debugging-port={}", self.debug_port))
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
            if is_port_open(self.debug_port) {
                return Ok(());
            }
        }

        self.shutdown().await;
        Err(RestoreError::ChromeConnectionFailed)
    }

    pub async fn shutdown(&mut self) {
        if let Some(mut child) = self.chrome_child.take() {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
        }
        self.temp_profile = None;
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
        if !target
            .url
            .as_deref()
            .unwrap_or_default()
            .contains("icloud.com")
        {
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
        let mut detected_docws: Option<String> = None;
        let mut pending_jar: Option<(CapturedRequestCredentials, String)> = None;
        let mut jar_capture_started: Option<Instant> = None;
        let mut cookie_request_ids: HashSet<u64> = HashSet::new();
        // Avoid CDP command id clashes with bootstrap calls 1..=3 above.
        let mut next_cdp_id: u64 = 10;
        let mut extra_headers_by_request_id: HashMap<String, String> = HashMap::new();

        fn finish_if_shard_and_jar_ready(
            pending_jar: &mut Option<(CapturedRequestCredentials, String)>,
            detected_docws: &Option<String>,
        ) -> Option<Credentials> {
            let (_, _) = pending_jar.as_ref()?;
            detected_docws.as_ref()?;
            let (cap, cookies) = pending_jar.take()?;
            Some(credentials_from_capture(
                cap,
                cookies,
                detected_docws.clone(),
            ))
        }

        /// After the cookie jar is ready, Apple may still be opening DocWS streams — avoid falling
        /// through to the wrong `-docws` pool for tens of seconds.
        const DOCWS_PATIENCE_AFTER_JAR_SECS: u64 = 75;

        while let Some(message) = ws.next().await {
            let message = message
                .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?;
            if !message.is_text() {
                continue;
            }

            let value: Value = serde_json::from_str(message.to_text().unwrap_or(""))
                .map_err(|e| RestoreError::Api(e.to_string()))?;

            note_docws_from_cdp_events(&mut detected_docws, &value);

            if let Some(done) = finish_if_shard_and_jar_ready(&mut pending_jar, &detected_docws) {
                return Ok(done);
            }

            if let Some(started) = jar_capture_started.as_ref() {
                if pending_jar.is_some()
                    && detected_docws.is_none()
                    && started.elapsed() >= Duration::from_secs(DOCWS_PATIENCE_AFTER_JAR_SECS)
                {
                    let (cap, cookies) = pending_jar.take().expect("pending_jar presence checked");
                    return Ok(credentials_from_capture(cap, cookies, None));
                }
            }

            if let Some((request_id, cookie_header)) = cookie_header_from_extra_info(&value) {
                if captured.is_some() && cookie_header_has_auth_signal(&cookie_header) {
                    request_cookie_snapshots(&mut ws, &mut cookie_request_ids, &mut next_cdp_id)
                        .await?;
                }
                extra_headers_by_request_id.insert(request_id, cookie_header);
            }

            if let Some((request_id, url, cookie_header)) = request_credentials_event_parts(&value)
            {
                let header_has_webauth = cookie_header
                    .as_ref()
                    .is_some_and(|h| cookie_header_has_auth_signal(h));

                if captured.is_some() && is_icloud_url(url) && header_has_webauth {
                    request_cookie_snapshots(&mut ws, &mut cookie_request_ids, &mut next_cdp_id)
                        .await?;
                }

                if captured.is_none() {
                    if let Some(next_capture) = capture_credentials_from_url(url) {
                        captured = Some(next_capture);

                        let mut trigger_second_wave = header_has_webauth;
                        if let Some(extra_cookie) = extra_headers_by_request_id.remove(&request_id)
                        {
                            if cookie_header_has_auth_signal(&extra_cookie) {
                                trigger_second_wave = true;
                            }
                        }

                        request_cookie_snapshots(
                            &mut ws,
                            &mut cookie_request_ids,
                            &mut next_cdp_id,
                        )
                        .await?;

                        // HttpOnly / session completes often trail the first authenticated XHR — a second
                        // snapshot reduces blank docws calls without trusting stripped request Cookie headers.
                        if trigger_second_wave {
                            request_cookie_snapshots(
                                &mut ws,
                                &mut cookie_request_ids,
                                &mut next_cdp_id,
                            )
                            .await?;
                        }
                    }
                }
            }

            if value
                .get("id")
                .and_then(Value::as_u64)
                .is_some_and(|id| cookie_request_ids.contains(&id))
            {
                if let Some(cookies) = parse_cdp_cookies(&value) {
                    if let Some(cap) = captured.clone() {
                        pending_jar = Some((cap, cookies));
                        jar_capture_started.get_or_insert(Instant::now());
                        if let Some(done) =
                            finish_if_shard_and_jar_ready(&mut pending_jar, &detected_docws)
                        {
                            return Ok(done);
                        }
                    }
                }
            }
        }

        Err(RestoreError::LoginTimeout)
    }

    async fn pick_page_target(&self) -> Result<CdpTarget, RestoreError> {
        let targets: Vec<CdpTarget> = self
            .client
            .get(format!("{}/json", self.debug_url()))
            .send()
            .await
            .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?
            .json()
            .await
            .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?;

        targets
            .iter()
            .find(|target| {
                target.target_type.as_deref() == Some("page")
                    && target.websocket_debugger_url.is_some()
                    && target.url.as_deref().is_some_and(is_icloud_url)
            })
            .cloned()
            .or_else(|| {
                targets.into_iter().find(|target| {
                    target.target_type.as_deref() == Some("page")
                        && target.websocket_debugger_url.is_some()
                })
            })
            .ok_or(RestoreError::ChromeConnectionFailed)
    }

    fn debug_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.debug_port)
    }
}

impl Drop for ChromeAuthenticator {
    fn drop(&mut self) {
        if let Some(mut child) = self.chrome_child.take() {
            let _ = child.start_kill();
        }
    }
}

trait RestoreErrorDetail {
    fn with_detail(self, detail: String) -> RestoreError;
}

impl RestoreErrorDetail for RestoreError {
    fn with_detail(self, detail: String) -> RestoreError {
        match self {
            RestoreError::ChromeConnectionFailed => {
                RestoreError::Api(format!("chrome_connection_failed:{detail}"))
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

fn allocate_debug_port() -> Result<u16, RestoreError> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))
}

async fn send_cdp<S>(ws: &mut S, id: u64, method: &str, params: Value) -> Result<(), RestoreError>
where
    S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
    <S as futures_util::Sink<tokio_tungstenite::tungstenite::Message>>::Error: std::fmt::Display,
{
    let message = json!({
        "id": id,
        "method": method,
        "params": params,
    });
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        message.to_string().into(),
    ))
    .await
    .map_err(|e| RestoreError::ChromeConnectionFailed.with_detail(e.to_string()))
}

async fn request_cookie_snapshots<S>(
    ws: &mut S,
    cookie_request_ids: &mut HashSet<u64>,
    next_cdp_id: &mut u64,
) -> Result<(), RestoreError>
where
    S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
    <S as futures_util::Sink<tokio_tungstenite::tungstenite::Message>>::Error: std::fmt::Display,
{
    let requests = [
        ("Network.getCookies", json!({ "urls": CDP_COOKIE_URLS })),
        ("Network.getAllCookies", json!({})),
        // Storage cookie schema differs across Chromium revisions; omit to avoid flaky parses.
    ];

    for (method, params) in requests {
        let id = *next_cdp_id;
        *next_cdp_id += 1;
        cookie_request_ids.insert(id);
        send_cdp(ws, id, method, params).await?;
    }

    Ok(())
}

/// URLs passed to CDP cookie snapshot — include apple.com because some session cookies attach there.
static CDP_COOKIE_URLS: &[&str] = &[
    "https://www.icloud.com",
    "https://icloud.com",
    "https://www.icloud.com/recovery/",
    "https://www.apple.com",
    "https://apple.com",
];

pub fn capture_credentials_from_url(url: &str) -> Option<CapturedRequestCredentials> {
    if !is_icloud_url(url) {
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
        client_build_number: client_build_number
            .unwrap_or_else(|| DEFAULT_CLIENT_BUILD.to_string()),
        client_mastering_number: client_mastering_number
            .unwrap_or_else(|| DEFAULT_CLIENT_BUILD.to_string()),
    })
}

fn credentials_from_capture(
    captured: CapturedRequestCredentials,
    cookies: String,
    docws_base_url: Option<String>,
) -> Credentials {
    Credentials {
        cookies,
        client_id: captured.client_id,
        dsid: captured.dsid,
        client_build_number: captured.client_build_number,
        client_mastering_number: captured.client_mastering_number,
        docws_base_url,
    }
}

fn note_docws_from_cdp_events(holder: &mut Option<String>, value: &Value) {
    if holder.is_some() {
        return;
    }
    if value.get("method").and_then(Value::as_str) != Some("Network.requestWillBeSent") {
        return;
    }
    let Some(raw) = value.pointer("/params/request/url").and_then(Value::as_str) else {
        return;
    };
    if let Some(origin) = docws_https_origin_from_url(raw) {
        *holder = Some(origin);
    }
}

fn docws_https_origin_from_url(raw: &str) -> Option<String> {
    let parsed = Url::parse(raw).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if !docws_shard_host_ok(&host) {
        return None;
    }
    Some(format!("https://{host}"))
}

fn docws_shard_host_ok(host: &str) -> bool {
    host.ends_with("-docws.icloud.com")
}

fn request_credentials_event_parts(value: &Value) -> Option<(String, &str, Option<String>)> {
    if value.get("method").and_then(Value::as_str) != Some("Network.requestWillBeSent") {
        return None;
    }

    let request_id = value
        .pointer("/params/requestId")
        .and_then(Value::as_str)?
        .to_string();
    let url = value
        .pointer("/params/request/url")
        .and_then(Value::as_str)?;
    let cookie_header = header_value(value.pointer("/params/request/headers")?, "cookie");

    Some((request_id, url, cookie_header))
}

fn cookie_header_from_extra_info(value: &Value) -> Option<(String, String)> {
    if value.get("method").and_then(Value::as_str) != Some("Network.requestWillBeSentExtraInfo") {
        return None;
    }

    let request_id = value
        .pointer("/params/requestId")
        .and_then(Value::as_str)?
        .to_string();
    let cookie_header = header_value(value.pointer("/params/headers")?, "cookie")?;

    Some((request_id, cookie_header))
}

fn header_value(headers: &Value, name: &str) -> Option<String> {
    let headers = headers.as_object()?;
    headers.iter().find_map(|(key, value)| {
        if key.eq_ignore_ascii_case(name) {
            value.as_str().map(ToString::to_string)
        } else {
            None
        }
    })
}

fn parse_cdp_cookies(value: &Value) -> Option<String> {
    let cookies = value.pointer("/result/cookies").and_then(Value::as_array)?;

    let cookie_header = cookies
        .iter()
        .filter_map(|cookie| {
            let name = cookie.get("name")?.as_str()?;
            let value = cookie.get("value")?.as_str()?;
            let domain = cookie.get("domain").and_then(Value::as_str);
            if domain.is_some_and(|domain| !cookie_domain_allowed_for_drive_api(domain)) {
                return None;
            }
            Some(format!("{name}={value}"))
        })
        .collect::<Vec<_>>()
        .join("; ");

    if cookie_header.is_empty() || !cookie_header_has_auth_signal(&cookie_header) {
        None
    } else {
        Some(cookie_header)
    }
}

fn cookie_header_has_auth_signal(cookie_header: &str) -> bool {
    cookie_header
        .split(';')
        .any(|part| part.trim_start().starts_with("X-APPLE-WEBAUTH"))
}

fn is_icloud_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(is_icloud_host))
        .unwrap_or(false)
}

fn is_icloud_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "icloud.com" || host.ends_with(".icloud.com")
}

fn cookie_domain_allowed_for_drive_api(domain: &str) -> bool {
    let domain = domain.trim_start_matches('.');
    let lower = domain.to_ascii_lowercase();
    is_icloud_host(&lower) || lower == "apple.com" || lower.ends_with(".apple.com")
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
        assert!(capture_credentials_from_url(
            "https://www.icloud.com.evil.test/?clientId=a&dsid=b"
        )
        .is_none());
    }

    #[test]
    fn parses_cdp_cookies_into_header() {
        let value = json!({
            "id": 4,
            "result": {
                "cookies": [
                    {"name": "X-APPLE-WEBAUTH", "value": "token"},
                    {"name": "b", "value": "2"}
                ]
            }
        });

        assert_eq!(
            parse_cdp_cookies(&value).unwrap(),
            "X-APPLE-WEBAUTH=token; b=2"
        );
    }

    #[test]
    fn cdp_cookie_parser_rejects_non_auth_cookies() {
        let value = json!({
            "id": 4,
            "result": {
                "cookies": [
                    {"name": "a", "value": "1", "domain": ".icloud.com"},
                    {"name": "b", "value": "2", "domain": ".icloud.com"}
                ]
            }
        });

        assert_eq!(parse_cdp_cookies(&value), None);
    }

    #[test]
    fn parses_cookie_header_from_authenticated_request() {
        let value = json!({
            "method": "Network.requestWillBeSent",
            "params": {
                "requestId": "123",
                "request": {
                    "url": "https://www.icloud.com/some/api?clientId=abc&dsid=123",
                    "headers": {
                        "Cookie": "X-APPLE-WEBAUTH=token; other=value"
                    }
                }
            }
        });

        let (request_id, url, cookie_header) = request_credentials_event_parts(&value).unwrap();

        assert_eq!(request_id, "123");
        assert!(url.contains("clientId=abc"));
        assert_eq!(cookie_header.unwrap(), "X-APPLE-WEBAUTH=token; other=value");
    }

    #[test]
    fn parses_cookie_header_from_extra_info() {
        let value = json!({
            "method": "Network.requestWillBeSentExtraInfo",
            "params": {
                "requestId": "extra-1",
                "headers": {
                    "cookie": "X-APPLE-WEBAUTH=token"
                }
            }
        });

        assert_eq!(
            cookie_header_from_extra_info(&value).unwrap(),
            ("extra-1".to_string(), "X-APPLE-WEBAUTH=token".to_string())
        );
    }

    #[test]
    fn cdp_cookie_parser_ignores_non_icloud_domains() {
        let value = json!({
            "id": 4,
            "result": {
                "cookies": [
                    {"name": "X-APPLE-WEBAUTH", "value": "token", "domain": ".icloud.com"},
                    {"name": "b", "value": "2", "domain": ".example.com"}
                ]
            }
        });

        assert_eq!(parse_cdp_cookies(&value).unwrap(), "X-APPLE-WEBAUTH=token");
    }

    #[test]
    fn docws_https_origin_prefers_https_docws_shard_hosts() {
        assert_eq!(
            docws_https_origin_from_url("https://p209-docws.icloud.com/document/get?dsid=x")
                .as_deref(),
            Some("https://p209-docws.icloud.com")
        );
        assert_eq!(
            docws_https_origin_from_url("HTTPS://p1-DOCWS.ICLOUD.COM/path").as_deref(),
            Some("https://p1-docws.icloud.com")
        );
    }

    #[test]
    fn docws_https_origin_rejects_non_https_and_fake_hosts() {
        assert!(docws_https_origin_from_url("http://p107-docws.icloud.com/").is_none());
        assert!(docws_https_origin_from_url("https://evil.com/p107-docws.icloud.com/").is_none());
        assert!(docws_https_origin_from_url("https://docws.icloud.com/").is_none());
        assert!(docws_https_origin_from_url("not a url").is_none());
    }

    #[test]
    fn auth_cookie_signal_requires_apple_webauth_cookie() {
        assert!(cookie_header_has_auth_signal(
            "X-APPLE-WEBAUTH=token; other=value"
        ));
        assert!(cookie_header_has_auth_signal(
            " other=value; X-APPLE-WEBAUTH-HSA-TRUST=token"
        ));
        assert!(!cookie_header_has_auth_signal(
            "other=value; session=not-enough"
        ));
    }
}
