use anyhow::{Context, Result, bail};
use reqwest::{Proxy, header::{CONTENT_DISPOSITION, HeaderMap, USER_AGENT}};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Mapping;

use crate::{ProfileExtra, VergeConfig};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportOptions {
    pub with_proxy: bool,
    pub self_proxy: bool,
    pub timeout_seconds: u64,
    pub danger_accept_invalid_certs: bool,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportResult {
    pub name: String,
    pub yaml: String,
    pub extra: Option<ProfileExtra>,
}

pub async fn import_subscription(url: &str, options: &ImportOptions, verge: &VergeConfig) -> Result<ImportResult> {
    let timeout_seconds = if options.timeout_seconds == 0 {
        20
    } else {
        options.timeout_seconds
    };

    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(std::time::Duration::from_secs(timeout_seconds))
        .redirect(reqwest::redirect::Policy::limited(10));

    if options.self_proxy {
        let proxy = format!("http://127.0.0.1:{}", verge.mixed_port);
        builder = builder.proxy(Proxy::all(&proxy).with_context(|| format!("invalid self proxy: {proxy}"))?);
    } else if !options.with_proxy {
        builder = builder.no_proxy();
    }

    if options.danger_accept_invalid_certs {
        builder = builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }

    let client = builder.build().context("build subscription client failed")?;
    let ua = options
        .user_agent
        .clone()
        .unwrap_or_else(|| "verge-tui/0.1.0".to_string());

    let response = client
        .get(url)
        .header(USER_AGENT, ua)
        .send()
        .await
        .with_context(|| format!("fetch subscription failed: {url}"))?;

    if !response.status().is_success() {
        bail!("subscription request failed with status {}", response.status());
    }

    let headers = response.headers().clone();
    let body = response.text().await.context("read subscription body failed")?;
    let body = body.trim_start_matches('\u{feff}').to_string();

    validate_yaml(&body)?;

    Ok(ImportResult {
        name: parse_name(url, &headers),
        yaml: body,
        extra: parse_subscription_userinfo(&headers),
    })
}

fn validate_yaml(content: &str) -> Result<()> {
    let yaml = serde_yaml_ng::from_str::<Mapping>(content).context("invalid YAML subscription")?;
    if !yaml.contains_key("proxies") && !yaml.contains_key("proxy-providers") {
        bail!("subscription does not contain proxies or proxy-providers");
    }
    Ok(())
}

fn parse_name(url: &str, headers: &HeaderMap) -> String {
    if let Some(disposition) = headers.get(CONTENT_DISPOSITION)
        && let Ok(value) = disposition.to_str()
    {
        for segment in value.split(';') {
            let segment = segment.trim();
            if let Some(file_name) = segment.strip_prefix("filename=") {
                return file_name.trim_matches('"').to_string();
            }
            if let Some(file_name) = segment.strip_prefix("filename*=") {
                let decoded = file_name.rsplit("''").next().unwrap_or(file_name);
                if let Ok(decoded) = urlencoding::decode(decoded) {
                    return decoded.to_string();
                }
            }
        }
    }

    let parsed = url::Url::parse(url)
        .ok()
        .and_then(|u| u.path_segments().and_then(|mut it| it.next_back().map(str::to_string)))
        .filter(|s| !s.is_empty());

    parsed.unwrap_or_else(|| "Remote Profile".to_string())
}

fn parse_subscription_userinfo(headers: &HeaderMap) -> Option<ProfileExtra> {
    let value = headers.iter().find_map(|(k, v)| {
        let key_lower = k.as_str().to_ascii_lowercase();
        if key_lower.ends_with("subscription-userinfo") {
            v.to_str().ok()
        } else {
            None
        }
    })?;

    Some(ProfileExtra {
        upload: parse_kv_u64(value, "upload").unwrap_or(0),
        download: parse_kv_u64(value, "download").unwrap_or(0),
        total: parse_kv_u64(value, "total").unwrap_or(0),
        expire: parse_kv_u64(value, "expire").unwrap_or(0),
    })
}

fn parse_kv_u64(input: &str, key: &str) -> Option<u64> {
    input
        .split(';')
        .find_map(|part| {
            let mut kv = part.splitn(2, '=');
            let k = kv.next()?.trim();
            let v = kv.next()?.trim();
            if k == key { v.parse::<u64>().ok() } else { None }
        })
}
