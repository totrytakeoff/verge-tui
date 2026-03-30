use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::subscription::{ImportOptions, import_subscription};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendExitPolicy {
    AlwaysOn,
    AlwaysOff,
    Query,
}

impl Default for BackendExitPolicy {
    fn default() -> Self {
        Self::Query
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VergeConfig {
    pub enable_system_proxy: bool,
    pub enable_tun_mode: bool,
    pub proxy_host: String,
    pub system_proxy_bypass: String,
    pub controller_url: String,
    pub secret: String,
    pub mixed_port: u16,
    pub default_delay_test_url: String,
    pub auto_update_subscription_minutes: u64,
    pub auto_cleanup_on_exit: bool,
    pub keep_core_on_exit: bool,
    pub backend_exit_policy: BackendExitPolicy,
}

impl Default for VergeConfig {
    fn default() -> Self {
        Self {
            enable_system_proxy: false,
            enable_tun_mode: false,
            proxy_host: "127.0.0.1".to_string(),
            system_proxy_bypass: String::new(),
            controller_url: "http://127.0.0.1:9097".to_string(),
            secret: String::new(),
            mixed_port: 7897,
            default_delay_test_url: "http://cp.cloudflare.com".to_string(),
            auto_update_subscription_minutes: 0,
            auto_cleanup_on_exit: true,
            keep_core_on_exit: true,
            backend_exit_policy: BackendExitPolicy::Query,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileExtra {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    pub expire: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileItem {
    pub uid: String,
    pub name: String,
    pub file: String,
    pub url: String,
    pub updated: u64,
    pub extra: Option<ProfileExtra>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppState {
    pub verge: VergeConfig,
    pub current: Option<String>,
    pub profiles: Vec<ProfileItem>,
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub state_file: PathBuf,
    pub profiles_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Result<Self> {
        let root = if let Ok(custom) = std::env::var("VERGE_TUI_HOME") {
            PathBuf::from(custom)
        } else {
            let home = dirs::home_dir().ok_or_else(|| anyhow!("home directory not found"))?;
            home.join(".config").join("verge-tui")
        };

        let state_file = root.join("state.yaml");
        let profiles_dir = root.join("profiles");
        Ok(Self {
            root,
            state_file,
            profiles_dir,
        })
    }

    pub async fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .await
            .with_context(|| format!("create root dir failed: {}", self.root.display()))?;
        fs::create_dir_all(&self.profiles_dir)
            .await
            .with_context(|| format!("create profiles dir failed: {}", self.profiles_dir.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct StateStore {
    pub paths: AppPaths,
    pub state: AppState,
}

impl StateStore {
    pub async fn load_or_init() -> Result<Self> {
        let paths = AppPaths::resolve()?;
        paths.ensure().await?;

        let state = if paths.state_file.exists() {
            let raw = fs::read_to_string(&paths.state_file)
                .await
                .with_context(|| format!("read state file failed: {}", paths.state_file.display()))?;
            serde_yaml_ng::from_str::<AppState>(&raw).context("parse state yaml failed")?
        } else {
            AppState::default()
        };

        let store = Self { paths, state };
        store.save().await?;
        Ok(store)
    }

    pub async fn save(&self) -> Result<()> {
        let yaml = serde_yaml_ng::to_string(&self.state).context("serialize state yaml failed")?;
        fs::write(&self.paths.state_file, yaml)
            .await
            .with_context(|| format!("write state file failed: {}", self.paths.state_file.display()))?;
        Ok(())
    }

    pub async fn import_profile(&mut self, url: &str, options: &ImportOptions) -> Result<ProfileItem> {
        let result = import_subscription(url, options, &self.state.verge).await?;

        let uid = format!("R{}", now_secs());
        let file = format!("{uid}.yaml");
        let profile_path = self.paths.profiles_dir.join(&file);
        fs::write(&profile_path, result.yaml.as_bytes())
            .await
            .with_context(|| format!("write profile failed: {}", profile_path.display()))?;

        let profile = ProfileItem {
            uid: uid.clone(),
            name: result.name,
            file,
            url: url.to_string(),
            updated: now_secs(),
            extra: result.extra,
        };

        self.state.profiles.push(profile.clone());
        if self.state.current.is_none() {
            self.state.current = Some(uid);
        }

        self.save().await?;
        Ok(profile)
    }

    pub async fn update_profile(&mut self, uid: &str, options: &ImportOptions) -> Result<ProfileItem> {
        let idx = self
            .state
            .profiles
            .iter()
            .position(|p| p.uid == uid)
            .ok_or_else(|| anyhow!("profile not found: {uid}"))?;

        let mut profile = self
            .state
            .profiles
            .get(idx)
            .cloned()
            .ok_or_else(|| anyhow!("profile not found at index: {idx}"))?;

        if profile.url.trim().is_empty() {
            return Err(anyhow!("profile has empty url: {uid}"));
        }

        let result = import_subscription(&profile.url, options, &self.state.verge).await?;
        let profile_path = self.paths.profiles_dir.join(&profile.file);
        fs::write(&profile_path, result.yaml.as_bytes())
            .await
            .with_context(|| format!("write profile failed: {}", profile_path.display()))?;

        // Keep user-visible name stable unless it is empty.
        if profile.name.trim().is_empty() {
            profile.name = result.name;
        }
        profile.extra = result.extra;
        profile.updated = now_secs();

        if let Some(slot) = self.state.profiles.get_mut(idx) {
            *slot = profile.clone();
        }
        self.save().await?;
        Ok(profile)
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
