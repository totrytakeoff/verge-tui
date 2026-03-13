use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use http::header::{CONNECTION, HOST, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE};
use reqwest::{
    Method,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
#[cfg(unix)]
use tokio::{net::UnixStream, time::sleep};
use tokio_tungstenite::{client_async, connect_async, tungstenite::handshake::client::generate_key};
use url::Url;

#[derive(Debug, Clone)]
enum Endpoint {
    Http { base_url: Url },
    LocalSocket { socket_path: String },
}

#[derive(Debug, Clone)]
pub struct MihomoClient {
    endpoint: Endpoint,
    secret: Option<String>,
    http: reqwest::Client,
}

impl MihomoClient {
    pub fn new(base_url: &str, secret: Option<&str>) -> Result<Self> {
        let base_url = Url::parse(base_url).with_context(|| format!("invalid controller URL: {base_url}"))?;
        let secret = secret.filter(|s| !s.is_empty()).map(std::string::ToString::to_string);
        let http = build_http_client(secret.as_deref(), None)?;
        Ok(Self {
            endpoint: Endpoint::Http { base_url },
            secret,
            http,
        })
    }

    pub fn new_local_socket(socket_path: &str) -> Result<Self> {
        let socket_path = socket_path.trim();
        if socket_path.is_empty() {
            return Err(anyhow!("empty socket path"));
        }
        let http = build_http_client(None, Some(socket_path))?;
        Ok(Self {
            endpoint: Endpoint::LocalSocket {
                socket_path: socket_path.to_string(),
            },
            secret: None,
            http,
        })
    }

    pub fn endpoint_label(&self) -> String {
        match &self.endpoint {
            Endpoint::Http { base_url } => base_url.to_string(),
            Endpoint::LocalSocket { socket_path } => format!("local://{socket_path}"),
        }
    }

    pub fn is_local_socket(&self) -> bool {
        matches!(self.endpoint, Endpoint::LocalSocket { .. })
    }

    pub async fn get_version(&self) -> Result<VersionResp> {
        self.get_json("/version").await
    }

    pub async fn get_base_config(&self) -> Result<Value> {
        self.get_json("/configs").await
    }

    pub async fn patch_base_config(&self, patch: &Value) -> Result<()> {
        self.request(Method::PATCH, "/configs")?
            .json(patch)
            .send()
            .await
            .context("patch /configs failed")?
            .error_for_status()
            .context("patch /configs returned error status")?;
        Ok(())
    }

    pub async fn reload_config_from_path(&self, path: &str, force: bool) -> Result<()> {
        let resp = self
            .request(Method::PUT, "/configs")?
            .query(&[("force", force)])
            .json(&serde_json::json!({ "path": path }))
            .send()
            .await
            .context("reload /configs failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| String::new());
            bail!("reload config failed with path: {path}, status: {status}, body: {body}");
        }
        Ok(())
    }

    pub async fn get_proxies(&self) -> Result<ProxiesResp> {
        self.get_json("/proxies").await
    }

    pub async fn select_node_for_group(&self, group_name: &str, proxy_name: &str) -> Result<()> {
        let group = urlencoding::encode(group_name);
        let path = format!("/proxies/{group}");

        self.request(Method::PUT, &path)?
            .json(&serde_json::json!({ "name": proxy_name }))
            .send()
            .await
            .with_context(|| format!("switch proxy failed: {group_name} -> {proxy_name}"))?
            .error_for_status()
            .with_context(|| format!("switch proxy status error: {group_name} -> {proxy_name}"))?;

        Ok(())
    }

    pub async fn delay_proxy_by_name(&self, proxy_name: &str, url: &str, timeout_ms: u64) -> Result<ProxyDelayResp> {
        let proxy_name = urlencoding::encode(proxy_name);
        let test_url = urlencoding::encode(url);
        let path = format!("/proxies/{proxy_name}/delay?url={test_url}&timeout={timeout_ms}");
        self.get_json(&path).await
    }

    pub async fn subscribe_traffic(&self) -> Result<mpsc::Receiver<TrafficResp>> {
        self.subscribe_json_ws("/traffic").await
    }

    pub async fn subscribe_connections(&self) -> Result<mpsc::Receiver<ConnectionsResp>> {
        self.subscribe_json_ws("/connections").await
    }

    async fn subscribe_json_ws<T>(&self, path: &str) -> Result<mpsc::Receiver<T>>
    where
        T: for<'de> Deserialize<'de> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel(256);

        match &self.endpoint {
            Endpoint::Http { .. } => {
                let ws_url = self.ws_url(path)?;
                let (ws, _) = connect_async(ws_url.as_str())
                    .await
                    .context("failed to connect mihomo websocket")?;

                let (_write, mut read) = ws.split();
                tokio::spawn(async move {
                    while let Some(message) = read.next().await {
                        let Ok(message) = message else {
                            break;
                        };
                        if !message.is_text() {
                            continue;
                        }

                        let Ok(text) = message.to_text() else {
                            continue;
                        };

                        let Ok(parsed) = serde_json::from_str::<T>(text) else {
                            continue;
                        };

                        if tx.send(parsed).await.is_err() {
                            break;
                        }
                    }
                });
            }
            Endpoint::LocalSocket { socket_path } => {
                #[cfg(unix)]
                {
                    let stream = connect_unix_socket(socket_path).await?;
                    let request = http::Request::builder()
                        .uri(format!("ws://localhost/{}", path.trim_start_matches('/')))
                        .header(HOST, "localhost")
                        .header(SEC_WEBSOCKET_KEY, generate_key())
                        .header(CONNECTION, "Upgrade")
                        .header(UPGRADE, "websocket")
                        .header(SEC_WEBSOCKET_VERSION, "13")
                        .body(())?;
                    let (ws, _) = client_async(request, stream)
                        .await
                        .context("failed to connect mihomo websocket by local socket")?;
                    let (_write, mut read) = ws.split();

                    tokio::spawn(async move {
                        while let Some(message) = read.next().await {
                            let Ok(message) = message else {
                                break;
                            };
                            if !message.is_text() {
                                continue;
                            }

                            let Ok(text) = message.to_text() else {
                                continue;
                            };

                            let Ok(parsed) = serde_json::from_str::<T>(text) else {
                                continue;
                            };

                            if tx.send(parsed).await.is_err() {
                                break;
                            }
                        }
                    });
                }

                #[cfg(not(unix))]
                {
                    let _ = path;
                    let _ = socket_path;
                    return Err(anyhow!("local socket websocket is unsupported on this platform"));
                }
            }
        }

        Ok(rx)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        self.request(Method::GET, path)?
            .send()
            .await
            .with_context(|| format!("GET {path} failed"))?
            .error_for_status()
            .with_context(|| format!("GET {path} returned error status"))?
            .json::<T>()
            .await
            .with_context(|| format!("decode response failed: {path}"))
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder> {
        let url = self.request_url(path)?;
        let req = match method {
            Method::GET => self.http.get(url),
            Method::POST => self.http.post(url),
            Method::PUT => self.http.put(url),
            Method::PATCH => self.http.patch(url),
            Method::DELETE => self.http.delete(url),
            _ => return Err(anyhow!("unsupported request method: {method}")),
        };
        Ok(req)
    }

    fn request_url(&self, path: &str) -> Result<String> {
        match &self.endpoint {
            Endpoint::Http { base_url } => base_url
                .join(path)
                .map(|u| u.to_string())
                .with_context(|| format!("failed to join path: {path}")),
            Endpoint::LocalSocket { .. } => {
                let path = path.trim_start_matches('/');
                Ok(format!("http://localhost/{path}"))
            }
        }
    }

    fn ws_url(&self, path: &str) -> Result<Url> {
        let mut ws = match &self.endpoint {
            Endpoint::Http { base_url } => base_url
                .join(path)
                .with_context(|| format!("failed to join websocket path: {path}"))?,
            Endpoint::LocalSocket { .. } => {
                return Err(anyhow!("ws_url is only for http endpoint"));
            }
        };

        let ws_scheme = match ws.scheme() {
            "http" => "ws",
            "https" => "wss",
            other => return Err(anyhow!("unsupported scheme for websocket: {other}")),
        };

        ws.set_scheme(ws_scheme)
            .map_err(|_| anyhow!("failed to set websocket scheme"))?;

        if let Some(secret) = self.secret.as_deref()
            && !secret.is_empty()
        {
            ws.query_pairs_mut().append_pair("token", secret);
        }
        Ok(ws)
    }
}

fn build_http_client(secret: Option<&str>, socket_path: Option<&str>) -> Result<reqwest::Client> {
    let mut headers = HeaderMap::new();
    if let Some(secret) = secret
        && !secret.is_empty()
    {
        let header = format!("Bearer {secret}");
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&header).context("invalid authorization header")?,
        );
    }

    let mut builder = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(8));

    if socket_path.is_none() {
        builder = builder.use_rustls_tls();
    }

    if let Some(socket_path) = socket_path {
        #[cfg(unix)]
        {
            builder = builder.unix_socket(socket_path);
        }
        #[cfg(windows)]
        {
            builder = builder.windows_named_pipe(socket_path);
        }
    }

    builder.build().context("failed to build mihomo HTTP client")
}

#[cfg(unix)]
async fn connect_unix_socket(socket_path: &str) -> Result<UnixStream> {
    let mut retries = 0u8;
    loop {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                retries += 1;
                if retries >= 3 {
                    return Err(err)
                        .with_context(|| format!("failed to connect unix socket after retries: {socket_path}"));
                }
                sleep(Duration::from_millis(80)).await;
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionResp {
    pub version: String,
    pub meta: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyDelayResp {
    pub delay: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxiesResp {
    pub proxies: HashMap<String, ProxyNode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyNode {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub now: Option<String>,
    pub all: Option<Vec<String>>,
    pub history: Option<Vec<ProxyHistory>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyHistory {
    pub delay: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrafficResp {
    pub up: u64,
    pub down: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConnectionsResp {
    #[serde(rename = "uploadTotal")]
    pub upload_total: u64,
    #[serde(rename = "downloadTotal")]
    pub download_total: u64,
}
