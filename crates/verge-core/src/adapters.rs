use anyhow::Result;
use sysproxy::Sysproxy;

use crate::VergeConfig;

#[cfg(target_os = "windows")]
const DEFAULT_BYPASS: &str =
    "localhost;127.*;192.168.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;<local>";
#[cfg(target_os = "linux")]
const DEFAULT_BYPASS: &str =
    "localhost,127.0.0.1,192.168.0.0/16,10.0.0.0/8,172.16.0.0/12,::1";
#[cfg(target_os = "macos")]
const DEFAULT_BYPASS: &str =
    "127.0.0.1,192.168.0.0/16,10.0.0.0/8,172.16.0.0/12,localhost,*.local,*.crashlytics.com,<local>";

pub fn apply_system_proxy(verge: &VergeConfig) -> Result<()> {
    let mut sys = Sysproxy {
        enable: verge.enable_system_proxy,
        host: verge.proxy_host.clone().into(),
        port: verge.mixed_port,
        bypass: if verge.system_proxy_bypass.is_empty() {
            DEFAULT_BYPASS.into()
        } else {
            format!("{DEFAULT_BYPASS},{}", verge.system_proxy_bypass).into()
        },
    };

    if !verge.enable_system_proxy {
        sys.enable = false;
    }

    sys.set_system_proxy()?;
    Ok(())
}
