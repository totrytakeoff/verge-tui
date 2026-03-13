use std::{
    collections::HashMap,
    collections::HashSet,
    collections::VecDeque,
    fs,
    fs::OpenOptions,
    io,
    io::Write as _,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clash_verge_service_ipc::{ClashConfig, CoreConfig, IpcConfig, WriterConfig};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mihomo_client::{ConnectionsResp, MihomoClient, ProxiesResp, TrafficResp};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Sparkline, Wrap},
};
use tokio::{process::Child, sync::mpsc, time::sleep};
use verge_core::{BackendExitPolicy, ImportOptions, StateStore, apply_system_proxy};

const LOG_LIMIT: usize = 200;
const TRAFFIC_HISTORY_LIMIT: usize = 120;
const MANAGED_PORT_FALLBACK_BASE: u16 = 17897;
const TUI_TUN_DEVICE: &str = "vergetui0";
// Keep UI responsive: do not block startup/ticks for long service IPC waits.
const SERVICE_WAIT_RETRIES: usize = 6;
const SERVICE_WAIT_INTERVAL_MS: u64 = 120;
const SERVICE_IPC_PRIMARY_PATH: &str = "/tmp/verge/clash-verge-service.sock";
const SERVICE_IPC_LEGACY_PATH: &str = "/tmp/clash-verge-service.sock";
const COLOR_BG: Color = Color::Rgb(16, 18, 22);
const COLOR_PANEL: Color = Color::Rgb(30, 36, 44);
const COLOR_TEXT: Color = Color::Rgb(215, 223, 232);
const COLOR_ACCENT: Color = Color::Rgb(123, 203, 172);
const COLOR_HOT: Color = Color::Rgb(243, 139, 168);
const COLOR_WARN: Color = Color::Rgb(245, 169, 127);

#[derive(Debug, Clone, Copy)]
enum Tab {
    Overview,
    Profiles,
    Proxies,
    Logs,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Overview, Tab::Profiles, Tab::Proxies, Tab::Logs];

    const fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Profiles => "Profiles",
            Tab::Proxies => "Proxies",
            Tab::Logs => "Logs",
        }
    }
}

#[derive(Debug, Clone)]
struct ProxyGroupView {
    name: String,
    kind: String,
    now: String,
    candidates: Vec<String>,
}

#[derive(Debug, Clone)]
struct ClashVergeApiHint {
    controller_url: Option<String>,
    socket_path: Option<String>,
    secret: Option<String>,
    mixed_port: Option<u16>,
    enable_external_controller: Option<bool>,
    clash_core: Option<String>,
    app_home: PathBuf,
    source_config: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyFocus {
    Groups,
    Candidates,
}

#[derive(Debug)]
enum BulkDelayEvent {
    Started {
        total: usize,
        url: String,
        timeout_ms: u64,
    },
    Item {
        node: String,
        delay: Option<u64>,
        error: Option<String>,
    },
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitConfirmChoice {
    KeepBackend,
    StopBackend,
}

struct App {
    tab_index: usize,
    command_mode: bool,
    command_input: String,
    show_help_overlay: bool,
    show_exit_confirm_overlay: bool,
    exit_confirm_choice: ExitConfirmChoice,
    exit_keep_backend_override: Option<bool>,
    should_quit: bool,
    bootstrap_pending: bool,
    app_started_at: Instant,
    tick_count: u64,

    store: StateStore,
    mihomo: MihomoClient,

    proxies: Option<ProxiesResp>,
    proxy_groups: Vec<ProxyGroupView>,
    selected_group_idx: usize,
    selected_proxy_idx: usize,
    proxy_focus: ProxyFocus,
    selected_profile_idx: usize,
    traffic: TrafficResp,
    connections: ConnectionsResp,
    traffic_up_history: VecDeque<u64>,
    traffic_down_history: VecDeque<u64>,
    delay_cache: HashMap<String, u64>,

    traffic_rx: Option<mpsc::Receiver<TrafficResp>>,
    connections_rx: Option<mpsc::Receiver<ConnectionsResp>>,
    bulk_delay_rx: Option<mpsc::Receiver<BulkDelayEvent>>,
    bulk_delay_running: bool,
    bulk_delay_total: usize,
    bulk_delay_done: usize,
    bulk_delay_success: usize,
    bulk_delay_failed: usize,
    bulk_delay_url: String,
    bulk_delay_timeout_ms: u64,
    managed_core_child: Option<Child>,
    managed_core_socket: Option<String>,
    health_probe_failures: u8,
    direct_mode_logged: bool,
    auto_update_next_at: Option<Instant>,
    auto_update_running: bool,
    file_log_path: Option<PathBuf>,
    session_log_path: Option<PathBuf>,
    file_log: Option<std::fs::File>,
    session_log: Option<std::fs::File>,

    logs: VecDeque<String>,
}

impl App {
    async fn new() -> Result<Self> {
        clash_verge_service_ipc::set_config(Some(IpcConfig {
            // UI-first config: fail fast, retry in app loop instead of blocking current frame.
            default_timeout: Duration::from_millis(120),
            max_retries: 1,
            retry_delay: Duration::from_millis(80),
        }))
        .await;

        let store = StateStore::load_or_init().await?;
        let mihomo = MihomoClient::new(&store.state.verge.controller_url, Some(&store.state.verge.secret))?;

        let mut app = Self {
            tab_index: 0,
            command_mode: false,
            command_input: String::new(),
            show_help_overlay: false,
            show_exit_confirm_overlay: false,
            exit_confirm_choice: ExitConfirmChoice::KeepBackend,
            exit_keep_backend_override: None,
            should_quit: false,
            bootstrap_pending: true,
            app_started_at: Instant::now(),
            tick_count: 0,
            store,
            mihomo,
            proxies: None,
            proxy_groups: Vec::new(),
            selected_group_idx: 0,
            selected_proxy_idx: 0,
            proxy_focus: ProxyFocus::Groups,
            selected_profile_idx: 0,
            traffic: TrafficResp { up: 0, down: 0 },
            connections: ConnectionsResp {
                upload_total: 0,
                download_total: 0,
            },
            traffic_up_history: VecDeque::with_capacity(TRAFFIC_HISTORY_LIMIT),
            traffic_down_history: VecDeque::with_capacity(TRAFFIC_HISTORY_LIMIT),
            delay_cache: HashMap::new(),
            traffic_rx: None,
            connections_rx: None,
            bulk_delay_rx: None,
            bulk_delay_running: false,
            bulk_delay_total: 0,
            bulk_delay_done: 0,
            bulk_delay_success: 0,
            bulk_delay_failed: 0,
            bulk_delay_url: String::new(),
            bulk_delay_timeout_ms: 0,
            managed_core_child: None,
            managed_core_socket: None,
            health_probe_failures: 0,
            direct_mode_logged: false,
            auto_update_next_at: None,
            auto_update_running: false,
            file_log_path: None,
            session_log_path: None,
            file_log: None,
            session_log: None,
            logs: VecDeque::new(),
        };

        app.init_file_logging();
        app.log_boot_diagnostics();
        app.schedule_next_auto_update("startup");
        app.push_log("verge-tui started. Type ':' then 'help' to view commands.");
        if let Err(err) = apply_system_proxy(&app.store.state.verge) {
            app.push_log(format!("apply system proxy on boot failed: {err}"));
        }

        app.push_log("startup defer: core/bootstrap will continue in background ticks");
        Ok(app)
    }

    async fn recreate_client(&mut self) -> Result<()> {
        self.mihomo = MihomoClient::new(
            &self.store.state.verge.controller_url,
            Some(&self.store.state.verge.secret),
        )?;
        let healthy = self.ensure_mihomo_ready(true).await;
        if healthy {
            self.apply_current_profile_to_core().await;
            self.init_streams().await;
        }
        Ok(())
    }

    fn init_file_logging(&mut self) {
        let logs_dir = self.store.paths.root.join("logs");
        if let Err(err) = fs::create_dir_all(&logs_dir) {
            self.logs
                .push_back(format!("[{}] create log dir failed: {err}", epoch_hms()));
            return;
        }

        let main_log = logs_dir.join("verge-tui.log");
        let session_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let session_log = logs_dir.join(format!("session-{session_ts}.log"));

        self.file_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&main_log)
            .ok();
        self.session_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&session_log)
            .ok();
        self.file_log_path = Some(main_log);
        self.session_log_path = Some(session_log);
    }

    fn log_boot_diagnostics(&mut self) {
        let ctl = self.store.state.verge.controller_url.clone();
        let mixed = self.store.state.verge.mixed_port;
        let tun = self.store.state.verge.enable_tun_mode;
        let sys = self.store.state.verge.enable_system_proxy;
        let root = self.store.paths.root.display().to_string();
        let profiles = self.store.state.profiles.len();
        let current = self.store.state.current.clone().unwrap_or_else(|| "-".to_string());
        self.push_log(format!(
            "startup: root={root}, profiles={profiles}, current={current}, controller={ctl}, mixed-port={mixed}, sys={sys}, tun={tun}"
        ));
        self.push_log(format!(
            "startup mode: independent-core={}, service-ipc={}",
            prefers_independent_core(),
            prefers_service_ipc()
        ));
        self.push_log(format!(
            "startup policy: autosub={}min cleanup-on-exit={} backend-exit-policy={} (legacy keep-core-on-exit={})",
            self.store.state.verge.auto_update_subscription_minutes,
            self.store.state.verge.auto_cleanup_on_exit,
            backend_exit_policy_label(self.store.state.verge.backend_exit_policy),
            self.store.state.verge.keep_core_on_exit
        ));
        if let Some(path) = self.file_log_path.as_ref() {
            self.push_log(format!("file log: {}", path.display()));
        }
        if let Some(path) = self.session_log_path.as_ref() {
            self.push_log(format!("session log: {}", path.display()));
        }
    }

    async fn probe_mihomo_health(&self) -> bool {
        self.mihomo.get_version().await.is_ok()
    }

    async fn check_mihomo_health(&mut self) -> bool {
        let endpoint = self.mihomo.endpoint_label();
        match self.mihomo.get_version().await {
            Ok(version) => {
                self.push_log(format!("mihomo controller online ({endpoint}): {}", version.version));
                true
            }
            Err(err) => {
                self.push_log(format!(
                    "mihomo controller unreachable ({endpoint}): {err}. check core process, external-controller, secret, or local socket path",
                ));
                false
            }
        }
    }

    async fn apply_current_profile_to_core(&mut self) {
        let Some(uid) = self.store.state.current.clone() else {
            return;
        };
        let Some(profile) = self.store.state.profiles.iter().find(|p| p.uid == uid) else {
            return;
        };
        let file = profile.file.clone();
        let name = profile.name.clone();
        let path = self.store.paths.profiles_dir.join(&file);
        if !path.exists() {
            self.push_log(format!("skip apply current profile: missing file {}", path.display()));
            return;
        }
        match self.apply_profile_file_to_core(&file).await {
            Ok(_) => self.push_log(format!("applied current profile to mihomo: {name}")),
            Err(err) => self.push_log(format!("apply current profile failed: {err}")),
        }
    }

    async fn apply_profile_file_to_core(&self, file: &str) -> Result<()> {
        let path = self.store.paths.profiles_dir.join(file);
        if !path.exists() {
            bail!("profile file not found: {}", path.display());
        }
        let path_str = path.to_string_lossy().to_string();
        let reload_res = match self.mihomo.reload_config_from_path(&path_str, true).await {
            Ok(()) => Ok(()),
            Err(err) => {
                let msg = err.to_string();
                if msg.contains("SAFE_PATHS") || msg.contains("path is not subpath of home directory") {
                    let copied = self.copy_profile_to_safe_path(file, &path)?;
                    let copied_str = copied.to_string_lossy().to_string();
                    return self
                        .mihomo
                        .reload_config_from_path(&copied_str, true)
                        .await
                        .with_context(|| {
                            format!(
                                "reload failed after safe-path copy. src={}, copied={}",
                                path.display(),
                                copied.display()
                            )
                        });
                }
                Err(err)
            }
        };

        reload_res?;
        self.ensure_runtime_port_alignment().await?;
        Ok(())
    }

    async fn ensure_runtime_port_alignment(&self) -> Result<()> {
        let desired = self.store.state.verge.mixed_port;
        let current = self
            .mihomo
            .get_base_config()
            .await
            .ok()
            .and_then(|cfg| cfg.get("mixed-port").and_then(|v| v.as_u64()))
            .and_then(|v| u16::try_from(v).ok());

        if current == Some(desired) {
            return Ok(());
        }

        self.mihomo
            .patch_base_config(&serde_json::json!({
                "mixed-port": desired
            }))
            .await
            .with_context(|| {
                format!(
                    "runtime mixed-port alignment failed: desired={desired}, current={:?}",
                    current
                )
            })?;
        Ok(())
    }

    fn copy_profile_to_safe_path(&self, file: &str, src: &Path) -> Result<PathBuf> {
        let mut candidates = vec![self.store.paths.root.join("core-home").join("verge-tui-profiles")];
        if let Some(hint) = detect_clash_verge_api_hint() {
            candidates.push(hint.app_home.join("verge-tui-profiles"));
        }

        let data = std::fs::read(src).with_context(|| format!("read profile source failed: {}", src.display()))?;
        let mut last_err = String::new();
        for dir in candidates {
            match std::fs::create_dir_all(&dir) {
                Ok(_) => {
                    let target = dir.join(file);
                    match std::fs::write(&target, &data) {
                        Ok(_) => return Ok(target),
                        Err(err) => {
                            last_err = format!("write {} failed: {err}", target.display());
                        }
                    }
                }
                Err(err) => {
                    last_err = format!("create {} failed: {err}", dir.display());
                }
            }
        }

        bail!("write safe-path profile failed: {last_err}");
    }

    async fn ensure_mihomo_ready(&mut self, log_health: bool) -> bool {
        let independent = prefers_independent_core();

        if self.try_adopt_existing_local_socket().await {
            if log_health {
                return self.check_mihomo_health().await;
            }
            return self.probe_mihomo_health().await;
        }

        if !independent {
            if log_health {
                if self.check_mihomo_health().await {
                    return true;
                }
            } else if self.probe_mihomo_health().await {
                return true;
            }
        } else if self.mihomo.is_local_socket() {
            if log_health {
                if self.check_mihomo_health().await {
                    return true;
                }
            } else if self.probe_mihomo_health().await {
                return true;
            }
        } else {
            self.push_log("independent mode enabled: skip adopting external HTTP controller");
        }

        if let Err(err) = self.try_start_managed_mihomo().await {
            self.push_log(format!("start managed mihomo failed: {err}"));
            if !independent {
                self.try_adopt_clash_verge_controller().await;
            }
        }

        if log_health {
            if self.check_mihomo_health().await {
                return true;
            }
        } else {
            if self.probe_mihomo_health().await {
                return true;
            }
        }

        if !independent {
            self.try_adopt_clash_verge_controller().await;
            if log_health {
                return self.check_mihomo_health().await;
            }
            return self.probe_mihomo_health().await;
        }

        false
    }

    async fn try_adopt_existing_local_socket(&mut self) -> bool {
        let mut candidates = vec![
            "/var/tmp/verge/verge-mihomo.sock".to_string(),
            "/tmp/verge/verge-mihomo.sock".to_string(),
            "/tmp/verge-tui/verge-mihomo.sock".to_string(),
        ];

        if let Some(hint) = detect_clash_verge_api_hint() {
            for c in local_socket_candidates(&hint) {
                candidates.push(c);
            }
        }

        let mut seen = HashSet::new();
        for socket_path in candidates {
            if !seen.insert(socket_path.clone()) {
                continue;
            }
            if !Path::new(&socket_path).exists() {
                continue;
            }

            let Ok(client) = MihomoClient::new_local_socket(&socket_path) else {
                continue;
            };
            let Ok(v) = client.get_version().await else {
                continue;
            };

            let current = self.mihomo.endpoint_label();
            let target = format!("local://{socket_path}");
            self.mihomo = client;
            if current != target {
                self.push_log(format!(
                    "adopted existing mihomo socket => {socket_path} (mihomo {})",
                    v.version
                ));
            }
            return true;
        }
        false
    }

    async fn try_adopt_clash_verge_controller(&mut self) {
        let Some(hint) = detect_clash_verge_api_hint() else {
            self.push_log("no clash-verge config.yaml found for controller auto-detect");
            return;
        };

        self.push_log(format!("detected clash-verge config: {}", hint.source_config.display()));

        if hint.enable_external_controller == Some(false) || hint.controller_url.is_none() {
            for socket_path in local_socket_candidates(&hint) {
                if !Path::new(&socket_path).exists() {
                    continue;
                }

                match MihomoClient::new_local_socket(&socket_path) {
                    Ok(client) => match client.get_version().await {
                        Ok(v) => {
                            self.mihomo = client;
                            self.push_log(format!("adopted local socket => {socket_path} (mihomo {})", v.version));
                            return;
                        }
                        Err(err) => {
                            self.push_log(format!("socket candidate unreachable: {socket_path} ({err})"));
                        }
                    },
                    Err(err) => {
                        self.push_log(format!("adopt local socket failed: {socket_path} ({err})"));
                    }
                }
            }
        }

        let Some(controller_url) = hint.controller_url.clone() else {
            if hint.enable_external_controller == Some(false) {
                self.push_log("clash-verge external-controller is disabled, and no local socket path was reachable");
            } else {
                self.push_log("clash-verge external-controller is empty; cannot connect by HTTP API");
            }
            return;
        };

        let old_url = self.store.state.verge.controller_url.clone();
        let old_secret = self.store.state.verge.secret.clone();
        let old_port = self.store.state.verge.mixed_port;

        self.store.state.verge.controller_url = controller_url;
        if let Some(secret) = hint.secret {
            self.store.state.verge.secret = secret;
        }
        if let Some(mixed_port) = hint.mixed_port {
            self.store.state.verge.mixed_port = mixed_port;
        }

        if old_url == self.store.state.verge.controller_url
            && old_secret == self.store.state.verge.secret
            && old_port == self.store.state.verge.mixed_port
        {
            self.push_log("controller auto-detect found same settings; no update");
            return;
        }

        if let Err(err) = self.store.save().await {
            self.push_log(format!("save auto-detected controller failed: {err}"));
            return;
        }

        match MihomoClient::new(
            &self.store.state.verge.controller_url,
            Some(&self.store.state.verge.secret),
        ) {
            Ok(client) => {
                self.mihomo = client;
                self.push_log(format!(
                    "adopted controller => {}",
                    self.store.state.verge.controller_url
                ));
            }
            Err(err) => {
                self.push_log(format!("build detected controller client failed: {err}"));
            }
        }
    }

    fn reap_managed_core(&mut self) {
        let outcome = match self.managed_core_child.as_mut() {
            Some(child) => child.try_wait(),
            None => return,
        };

        match outcome {
            Ok(Some(status)) => {
                self.managed_core_child = None;
                self.managed_core_socket = None;
                self.remove_managed_core_pid_file();
                self.push_log(format!("managed mihomo exited: {status}"));
            }
            Ok(None) => {}
            Err(err) => {
                self.managed_core_child = None;
                self.managed_core_socket = None;
                self.remove_managed_core_pid_file();
                self.push_log(format!("managed mihomo wait failed: {err}"));
            }
        }
    }

    async fn try_start_managed_mihomo(&mut self) -> Result<()> {
        self.reap_managed_core();
        if self.managed_core_child.is_some() {
            return Ok(());
        }
        if let Some(pid) = self.read_managed_core_pid()
            && !is_pid_alive(pid)
        {
            self.remove_managed_core_pid_file();
            self.push_log(format!("removed stale managed core pid file (pid={pid})"));
        }

        let use_service_ipc = prefers_service_ipc();
        if use_service_ipc {
            let _ = self.try_bootstrap_service_ipc().await;
        } else if !self.direct_mode_logged {
            self.push_log("direct-core mode: skip service IPC (set VERGE_TUI_USE_SERVICE_IPC=1 to enable)");
            self.direct_mode_logged = true;
        }

        let independent = prefers_independent_core();
        let hint = detect_clash_verge_api_hint();
        let socket = pick_socket_for_spawn(hint.as_ref(), independent);
        if let Some(parent) = Path::new(&socket).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        #[cfg(unix)]
        {
            if Path::new(&socket).exists() {
                let _ = std::fs::remove_file(&socket);
            }
        }

        let preferred_core = hint.as_ref().and_then(|h| h.clash_core.as_deref());
        let Some(core_bin) = resolve_core_binary(preferred_core) else {
            self.push_log(
                "cannot auto-start mihomo: no executable found (set VERGE_TUI_CORE_BIN or install verge-mihomo)",
            );
            return Ok(());
        };

        let (config_home, cfg) = self.prepare_runtime_config_for_managed_core(hint.as_ref(), &socket, independent)?;

        if use_service_ipc {
            if self
                .try_start_managed_mihomo_by_service(&core_bin, &config_home, &cfg, &socket)
                .await?
            {
                return Ok(());
            }
        }

        if self.store.state.verge.enable_tun_mode {
            self.push_log(
                "starting direct core in tun mode; if tun fails, grant NET_ADMIN/NET_RAW to verge-mihomo or enable service IPC",
            );
            if let Some(has_caps) = core_has_linux_tun_caps(&core_bin) {
                if !has_caps {
                    self.push_log(tun_privilege_hint());
                }
            }
        }

        self.start_managed_mihomo_by_child(&core_bin, &config_home, &cfg, &socket)
            .await?;
        Ok(())
    }

    fn prepare_runtime_config_for_managed_core(
        &mut self,
        hint: Option<&ClashVergeApiHint>,
        socket: &str,
        independent: bool,
    ) -> Result<(PathBuf, PathBuf)> {
        if !independent && let Some(hint) = hint {
            let runtime_cfg = hint.app_home.join("clash-verge.yaml");
            if runtime_cfg.exists() {
                return Ok((hint.app_home.clone(), runtime_cfg));
            }
            if hint.source_config.exists() {
                return Ok((hint.app_home.clone(), hint.source_config.clone()));
            }
        }

        let profile_path = if let Some(current) = self.store.state.current.as_deref() {
            self.store
                .state
                .profiles
                .iter()
                .find(|p| p.uid == current)
                .map(|p| self.store.paths.profiles_dir.join(&p.file))
        } else {
            self.store
                .state
                .profiles
                .first()
                .map(|p| self.store.paths.profiles_dir.join(&p.file))
        };
        let profile_path = if let Some(path) = profile_path {
            path
        } else if let Some(hint) = hint {
            self.push_log(format!(
                "no TUI profile found; fallback runtime config => {}",
                hint.source_config.display()
            ));
            return Ok((hint.app_home.clone(), hint.source_config.clone()));
        } else {
            return Err(anyhow::anyhow!("no profile available to build runtime config"));
        };
        if !profile_path.exists() {
            if let Some(hint) = hint {
                self.push_log(format!(
                    "selected TUI profile file missing: {}. fallback runtime config => {}",
                    profile_path.display(),
                    hint.source_config.display()
                ));
                return Ok((hint.app_home.clone(), hint.source_config.clone()));
            }
            return Err(anyhow::anyhow!(
                "selected TUI profile file missing: {}",
                profile_path.display()
            ));
        }

        let raw = std::fs::read_to_string(&profile_path)
            .with_context(|| format!("read profile failed: {}", profile_path.display()))?;
        let mut map = serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(&raw)
            .with_context(|| format!("parse yaml failed: {}", profile_path.display()))?;

        let runtime_mixed = pick_runtime_mixed_port(self.store.state.verge.mixed_port);
        if runtime_mixed != self.store.state.verge.mixed_port {
            self.push_log(format!(
                "managed core mixed-port adjusted: {} -> {} (port conflict)",
                self.store.state.verge.mixed_port, runtime_mixed
            ));
            self.store.state.verge.mixed_port = runtime_mixed;
        }
        map.insert("mixed-port".into(), runtime_mixed.into());
        map.insert("socks-port".into(), runtime_mixed.saturating_add(1).into());
        map.insert("port".into(), runtime_mixed.saturating_add(2).into());
        #[cfg(not(target_os = "windows"))]
        map.insert("redir-port".into(), runtime_mixed.saturating_add(3).into());
        #[cfg(target_os = "linux")]
        map.insert("tproxy-port".into(), runtime_mixed.saturating_add(4).into());
        map.insert("allow-lan".into(), false.into());
        map.insert("ipv6".into(), true.into());
        map.insert("mode".into(), "rule".into());
        map.insert("log-level".into(), "info".into());
        ensure_tun_defaults_in_mapping(&mut map, false);
        map.insert(
            "external-controller".into(),
            format!("127.0.0.1:{}", self.store.state.verge.mixed_port.saturating_add(1200)).into(),
        );
        #[cfg(unix)]
        map.insert("external-controller-unix".into(), socket.into());
        #[cfg(windows)]
        map.insert("external-controller-pipe".into(), socket.into());
        if !self.store.state.verge.secret.is_empty() {
            map.insert("secret".into(), self.store.state.verge.secret.clone().into());
        }
        let config_home = self.store.paths.root.join("core-home");
        std::fs::create_dir_all(&config_home)
            .with_context(|| format!("create core-home failed: {}", config_home.display()))?;
        let cfg = config_home.join("verge-tui-runtime.yaml");
        let yaml = serde_yaml_ng::to_string(&map).context("serialize runtime yaml failed")?;
        std::fs::write(&cfg, yaml).with_context(|| format!("write runtime yaml failed: {}", cfg.display()))?;
        self.push_log(format!("using independent runtime config: {}", cfg.display()));
        Ok((config_home, cfg))
    }

    async fn try_bootstrap_service_ipc(&mut self) -> bool {
        self.ensure_service_ipc_compat_path();
        if clash_verge_service_ipc::is_ipc_path_exists() {
            return self.wait_service_ipc_ready().await;
        }

        let service_sock = Path::new(SERVICE_IPC_PRIMARY_PATH);
        if let Some(parent) = service_sock.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if find_executable_in_path("systemctl").is_some() {
            let (sudo_ok, sudo_msg) =
                run_command_probe("sudo", &["-n", "systemctl", "start", "clash-verge-service.service"]);
            self.push_log(format!("service probe: {sudo_msg}"));
            if sudo_ok {
                self.push_log("requested start for clash-verge-service.service");
            } else {
                self.push_log("service needs privilege. run once: sudo systemctl start clash-verge-service.service");
            }
        }

        if !clash_verge_service_ipc::is_ipc_path_exists() {
            if let Some(service_bin) = resolve_service_binary() {
                self.push_log(format!(
                    "service binary detected but not started directly (needs root service): {}",
                    service_bin.display()
                ));
            }
        }

        if self.wait_service_ipc_ready().await {
            return true;
        }

        self.push_log("service IPC not ready; fallback to user-mode core process");
        false
    }

    async fn wait_service_ipc_ready(&mut self) -> bool {
        let mut last_err: Option<String> = None;
        for _ in 0..SERVICE_WAIT_RETRIES {
            self.ensure_service_ipc_compat_path();
            if clash_verge_service_ipc::is_ipc_path_exists() {
                match clash_verge_service_ipc::connect().await {
                    Ok(_) => {
                        self.push_log("clash-verge-service IPC is ready");
                        return true;
                    }
                    Err(err) => {
                        last_err = Some(err.to_string());
                    }
                }
            }
            sleep(Duration::from_millis(SERVICE_WAIT_INTERVAL_MS)).await;
        }

        #[cfg(unix)]
        {
            let primary_exists = Path::new(SERVICE_IPC_PRIMARY_PATH).exists();
            let legacy_exists = Path::new(SERVICE_IPC_LEGACY_PATH).exists();
            self.push_log(format!(
                "service IPC wait timeout: primary={primary_exists}, legacy={legacy_exists}, last_err={}",
                last_err.unwrap_or_else(|| "-".to_string())
            ));
        }
        #[cfg(not(unix))]
        {
            self.push_log(format!(
                "service IPC wait timeout: last_err={}",
                last_err.unwrap_or_else(|| "-".to_string())
            ));
        }

        false
    }

    fn ensure_service_ipc_compat_path(&mut self) {
        #[cfg(unix)]
        {
            let primary = Path::new(SERVICE_IPC_PRIMARY_PATH);
            if primary.exists() {
                return;
            }

            let legacy = Path::new(SERVICE_IPC_LEGACY_PATH);
            if !legacy.exists() {
                return;
            }

            if let Some(parent) = primary.parent() {
                if let Err(err) = fs::create_dir_all(parent) {
                    self.push_log(format!("service IPC compat mkdir failed: {} ({err})", parent.display()));
                    return;
                }
            }

            match std::os::unix::fs::symlink(legacy, primary) {
                Ok(_) => {
                    self.push_log(format!(
                        "service IPC compat linked: {} -> {}",
                        primary.display(),
                        legacy.display()
                    ));
                }
                Err(err) => {
                    if !primary.exists() {
                        self.push_log(format!(
                            "service IPC compat link failed: {} -> {} ({err})",
                            primary.display(),
                            legacy.display()
                        ));
                    }
                }
            }
        }
    }

    async fn try_start_managed_mihomo_by_service(
        &mut self,
        core_bin: &Path,
        config_home: &Path,
        cfg: &Path,
        socket: &str,
    ) -> Result<bool> {
        if !clash_verge_service_ipc::is_ipc_path_exists() {
            return Ok(false);
        }

        let logs_dir = config_home.join("logs");
        let _ = std::fs::create_dir_all(&logs_dir);
        let payload = ClashConfig {
            core_config: CoreConfig {
                core_path: core_bin.display().to_string(),
                core_ipc_path: socket.to_string(),
                config_path: cfg.display().to_string(),
                config_dir: config_home.display().to_string(),
            },
            log_config: WriterConfig {
                directory: logs_dir.display().to_string(),
                max_log_size: 8 * 1024 * 1024,
                max_log_files: 4,
            },
        };

        match clash_verge_service_ipc::start_clash(&payload).await {
            Ok(resp) => {
                if resp.code > 0 {
                    self.push_log(format!("service start core failed: {}", resp.message));
                    return Ok(false);
                }
                self.push_log("core start requested via clash-verge-service");
            }
            Err(err) => {
                self.push_log(format!("service ipc unavailable: {err}"));
                return Ok(false);
            }
        }

        for _ in 0..35 {
            sleep(Duration::from_millis(120)).await;
            if let Ok(client) = MihomoClient::new_local_socket(socket)
                && client.get_version().await.is_ok()
            {
                self.mihomo = client;
                self.managed_core_socket = Some(socket.to_string());
                self.remove_managed_core_pid_file();
                self.push_log(format!("managed mihomo ready by service on socket: {socket}"));
                return Ok(true);
            }
        }

        let http_controller = self.managed_http_controller_url();
        if let Ok(client) = MihomoClient::new(&http_controller, Some(&self.store.state.verge.secret))
            && client.get_version().await.is_ok()
        {
            self.mihomo = client;
            self.remove_managed_core_pid_file();
            self.push_log(format!(
                "managed mihomo ready by service on HTTP controller: {http_controller}"
            ));
            return Ok(true);
        }

        self.push_log(format!(
            "service launched core, but controller not ready in time (socket={socket}, http={http_controller})"
        ));
        Ok(false)
    }

    async fn start_managed_mihomo_by_child(
        &mut self,
        core_bin: &Path,
        config_home: &Path,
        cfg: &Path,
        socket: &str,
    ) -> Result<()> {
        let mut cmd = tokio::process::Command::new(core_bin);
        cmd.arg("-d").arg(config_home).arg("-f").arg(cfg);
        #[cfg(unix)]
        cmd.arg("-ext-ctl-unix").arg(socket);
        #[cfg(windows)]
        cmd.arg("-ext-ctl-pipe").arg(socket);

        let logs_dir = config_home.join("logs");
        let _ = std::fs::create_dir_all(&logs_dir);
        let core_log_path = logs_dir.join("managed-core.log");
        let stdout_log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&core_log_path)
            .with_context(|| format!("open managed core log failed: {}", core_log_path.display()))?;
        let stderr_log = stdout_log
            .try_clone()
            .with_context(|| format!("clone managed core log handle failed: {}", core_log_path.display()))?;

        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(stdout_log))
            .stderr(Stdio::from(stderr_log));

        let child = cmd.spawn().with_context(|| {
            format!(
                "spawn {} failed (home {}, config {})",
                core_bin.display(),
                config_home.display(),
                cfg.display()
            )
        })?;
        self.managed_core_child = Some(child);
        self.managed_core_socket = Some(socket.to_string());
        if let Some(pid) = self.managed_core_child.as_ref().and_then(|child| child.id()) {
            self.write_managed_core_pid_file(pid, socket, cfg);
            self.push_log(format!("managed core pid: {pid}"));
        }
        self.push_log(format!(
            "spawned managed mihomo: {} (config: {})",
            core_bin.display(),
            cfg.display()
        ));
        self.push_log(format!("managed core log file: {}", core_log_path.display()));

        for _ in 0..30 {
            sleep(Duration::from_millis(120)).await;
            self.reap_managed_core();
            if self.managed_core_child.is_none() {
                break;
            }
            if let Ok(client) = MihomoClient::new_local_socket(socket)
                && client.get_version().await.is_ok()
            {
                self.mihomo = client;
                self.push_log(format!("managed mihomo ready on socket: {socket}"));
                return Ok(());
            }
        }

        let http_controller = self.managed_http_controller_url();
        if let Ok(client) = MihomoClient::new(&http_controller, Some(&self.store.state.verge.secret))
            && client.get_version().await.is_ok()
        {
            self.mihomo = client;
            self.push_log(format!("managed mihomo ready on HTTP controller: {http_controller}"));
            return Ok(());
        }

        self.push_log(format!(
            "managed mihomo not ready (socket={socket}, http={http_controller}); check {}",
            core_log_path.display()
        ));
        for line in tail_text_lines(&core_log_path, 12) {
            self.push_log(format!("core-log> {line}"));
        }
        Ok(())
    }

    async fn shutdown(&mut self) {
        let keep_backend = self.effective_keep_core_on_exit();
        if keep_backend {
            self.push_log("exit: keeping backend running");
        } else {
            self.cleanup_before_exit().await;
            self.push_log("exit: stopping backend");
            self.stop_managed_core_backend().await;
        }

        if keep_backend {
            if let Some(child) = self.managed_core_child.take() {
                let pid = child.id().unwrap_or(0);
                self.push_log(format!("leaving managed core running in background (pid={pid})"));
                drop(child);
            }
            self.managed_core_socket = None;
        }
    }

    async fn init_streams(&mut self) {
        self.traffic_rx = match self.mihomo.subscribe_traffic().await {
            Ok(rx) => Some(rx),
            Err(err) => {
                self.push_log(format!("traffic ws unavailable: {err}"));
                None
            }
        };

        self.connections_rx = match self.mihomo.subscribe_connections().await {
            Ok(rx) => Some(rx),
            Err(err) => {
                self.push_log(format!("connections ws unavailable: {err}"));
                None
            }
        };
    }

    fn push_log(&mut self, msg: impl Into<String>) {
        if self.logs.len() >= LOG_LIMIT {
            let _ = self.logs.pop_front();
        }
        let stamp = epoch_hms();
        let line = format!("[{stamp}] {}", msg.into());
        self.logs.push_back(line.clone());
        self.write_log_line(&line);
    }

    fn write_log_line(&mut self, line: &str) {
        if let Some(file) = self.file_log.as_mut() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
        if let Some(file) = self.session_log.as_mut() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }

    fn effective_keep_core_on_exit(&self) -> bool {
        if let Some(keep) = self.exit_keep_backend_override {
            return keep;
        }

        match self.store.state.verge.backend_exit_policy {
            BackendExitPolicy::AlwaysOn => true,
            BackendExitPolicy::AlwaysOff => false,
            BackendExitPolicy::Query => self.store.state.verge.keep_core_on_exit,
        }
    }

    fn request_quit(&mut self) {
        self.exit_keep_backend_override = None;
        self.show_exit_confirm_overlay = false;
        match self.store.state.verge.backend_exit_policy {
            BackendExitPolicy::AlwaysOn => {
                self.exit_keep_backend_override = Some(true);
                self.should_quit = true;
            }
            BackendExitPolicy::AlwaysOff => {
                self.exit_keep_backend_override = Some(false);
                self.should_quit = true;
            }
            BackendExitPolicy::Query => {
                self.show_exit_confirm_overlay = true;
                self.exit_confirm_choice = if self.store.state.verge.keep_core_on_exit {
                    ExitConfirmChoice::KeepBackend
                } else {
                    ExitConfirmChoice::StopBackend
                };
            }
        }
    }

    async fn confirm_quit_by_choice(&mut self) {
        let keep = matches!(self.exit_confirm_choice, ExitConfirmChoice::KeepBackend);
        self.exit_keep_backend_override = Some(keep);
        self.store.state.verge.keep_core_on_exit = keep;
        if let Err(err) = self.store.save().await {
            self.push_log(format!("save exit choice failed: {err}"));
        }
        self.show_exit_confirm_overlay = false;
        self.should_quit = true;
    }

    fn managed_core_pid_file_path(&self) -> PathBuf {
        self.store.paths.root.join("core-home").join("managed-core.pid")
    }

    fn write_managed_core_pid_file(&mut self, pid: u32, socket: &str, cfg: &Path) {
        let pid_file = self.managed_core_pid_file_path();
        if let Some(parent) = pid_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content = format!(
            "pid={pid}\nsocket={socket}\nconfig={}\nupdated={}\n",
            cfg.display(),
            epoch_hms()
        );
        if let Err(err) = fs::write(&pid_file, content) {
            self.push_log(format!(
                "write managed core pid file failed: {} ({err})",
                pid_file.display()
            ));
        }
    }

    fn remove_managed_core_pid_file(&mut self) {
        let pid_file = self.managed_core_pid_file_path();
        let _ = fs::remove_file(pid_file);
    }

    fn read_managed_core_pid(&self) -> Option<u32> {
        let pid_file = self.managed_core_pid_file_path();
        let content = fs::read_to_string(pid_file).ok()?;
        for line in content.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("pid=")
                && let Ok(pid) = value.parse::<u32>()
            {
                return Some(pid);
            }
        }
        None
    }

    async fn stop_managed_core_backend(&mut self) {
        if let Some(mut child) = self.managed_core_child.take() {
            let pid = child.id().unwrap_or(0);
            self.push_log(format!("stopping managed core child (pid={pid})"));
            let _ = child.kill().await;
            let _ = child.wait().await;
            self.remove_managed_core_pid_file();
            self.managed_core_socket = None;
            return;
        }

        if let Some(pid) = self.read_managed_core_pid() {
            if terminate_pid(pid) {
                self.push_log(format!("sent terminate signal to managed core pid={pid}"));
                self.remove_managed_core_pid_file();
                self.managed_core_socket = None;
            } else {
                self.push_log(format!("failed to terminate pid={pid} (or already exited)"));
            }
            return;
        }

        self.push_log("managed core stop: no child handle and no pid file");
    }

    fn schedule_next_auto_update(&mut self, reason: &str) {
        let minutes = self.store.state.verge.auto_update_subscription_minutes;
        if minutes == 0 {
            self.auto_update_next_at = None;
            self.push_log(format!("auto subscription update disabled ({reason})"));
            return;
        }

        let delay = Duration::from_secs(minutes.saturating_mul(60));
        self.auto_update_next_at = Some(Instant::now() + delay);
        self.push_log(format!(
            "auto subscription update scheduled every {minutes} min ({reason})"
        ));
    }

    fn auto_update_status_line(&self) -> String {
        let minutes = self.store.state.verge.auto_update_subscription_minutes;
        if minutes == 0 {
            return "autosub: disabled".to_string();
        }
        let remain = self
            .auto_update_next_at
            .map(|at| at.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0);
        format!("autosub: every {minutes} min, next in {remain}s")
    }

    async fn maybe_run_auto_update(&mut self) {
        let Some(next) = self.auto_update_next_at else {
            return;
        };
        if self.auto_update_running || Instant::now() < next {
            return;
        }
        if self.store.state.profiles.is_empty() {
            self.push_log("auto subscription update skipped: no profiles");
            self.schedule_next_auto_update("no-profiles");
            return;
        }

        self.auto_update_running = true;
        self.push_log("auto subscription update started");
        self.refresh_all_profile_subscriptions().await;
        self.auto_update_running = false;
        self.schedule_next_auto_update("completed");
    }

    async fn cleanup_before_exit(&mut self) {
        if !self.store.state.verge.auto_cleanup_on_exit {
            self.push_log("exit cleanup disabled by config (set cleanup-on-exit on to enable)");
            return;
        }

        let mut state_changed = false;

        if self.store.state.verge.enable_tun_mode {
            match self.apply_tun_mode(false).await {
                Ok(_) => {
                    self.store.state.verge.enable_tun_mode = false;
                    state_changed = true;
                    self.push_log("exit cleanup: tun disabled");
                }
                Err(err) => {
                    self.push_log(format!("exit cleanup: disable tun failed: {err}"));
                    #[cfg(target_os = "linux")]
                    self.push_log("hint: run scripts/proxy-clean-linux.sh --yes if routes/rules remain");
                }
            }
        }

        let mut clear_proxy_cfg = self.store.state.verge.clone();
        clear_proxy_cfg.enable_system_proxy = false;
        match apply_system_proxy(&clear_proxy_cfg) {
            Ok(_) => {
                if self.store.state.verge.enable_system_proxy {
                    self.store.state.verge.enable_system_proxy = false;
                    state_changed = true;
                }
                self.push_log("exit cleanup: system proxy disabled");
            }
            Err(err) => {
                self.push_log(format!("exit cleanup: clear system proxy failed: {err}"));
            }
        }

        if state_changed && let Err(err) = self.store.save().await {
            self.push_log(format!("exit cleanup: save state failed: {err}"));
        }
    }

    async fn on_tick(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);
        self.reap_managed_core();

        if self.bootstrap_pending {
            self.bootstrap_pending = false;
            if self.ensure_mihomo_ready(false).await {
                self.apply_current_profile_to_core().await;
                self.refresh_proxies().await;
                self.init_streams().await;
                if self.store.state.verge.enable_tun_mode
                    && let Err(err) = self.apply_tun_mode(true).await
                {
                    self.push_log(format!("apply tun on boot failed: {err}"));
                }
            } else {
                self.push_log("mihomo api unavailable; skip proxy/traffic initialization");
            }
        }

        self.maybe_run_auto_update().await;
        if let Some(rx) = &mut self.traffic_rx {
            while let Ok(traffic) = rx.try_recv() {
                self.traffic = traffic;
            }
        }

        if let Some(rx) = &mut self.connections_rx {
            while let Ok(connections) = rx.try_recv() {
                self.connections = connections;
            }
        }

        let mut bulk_finished = false;
        let mut bulk_logs = Vec::new();
        if let Some(rx) = &mut self.bulk_delay_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    BulkDelayEvent::Started { total, url, timeout_ms } => {
                        self.bulk_delay_running = true;
                        self.bulk_delay_total = total;
                        self.bulk_delay_done = 0;
                        self.bulk_delay_success = 0;
                        self.bulk_delay_failed = 0;
                        self.bulk_delay_url = url.clone();
                        self.bulk_delay_timeout_ms = timeout_ms;
                        bulk_logs.push(format!(
                            "bulk delay started: {total} nodes, url={url}, timeout={timeout_ms}ms"
                        ));
                    }
                    BulkDelayEvent::Item { node, delay, error } => {
                        self.bulk_delay_done = self.bulk_delay_done.saturating_add(1);
                        if let Some(ms) = delay {
                            self.delay_cache.insert(node, ms);
                            self.bulk_delay_success = self.bulk_delay_success.saturating_add(1);
                        } else {
                            self.bulk_delay_failed = self.bulk_delay_failed.saturating_add(1);
                            if let Some(err) = error {
                                bulk_logs.push(format!("delay failed: {node} ({err})"));
                            }
                        }

                        if self.bulk_delay_done == self.bulk_delay_total || self.bulk_delay_done % 20 == 0 {
                            bulk_logs.push(format!(
                                "bulk delay progress: {}/{} (ok {}, fail {})",
                                self.bulk_delay_done,
                                self.bulk_delay_total,
                                self.bulk_delay_success,
                                self.bulk_delay_failed
                            ));
                        }
                    }
                    BulkDelayEvent::Finished => {
                        bulk_finished = true;
                    }
                }
            }
        }

        if bulk_finished {
            self.bulk_delay_running = false;
            self.bulk_delay_rx = None;
            bulk_logs.push(format!(
                "bulk delay finished: {}/{} ok, {} failed",
                self.bulk_delay_success, self.bulk_delay_total, self.bulk_delay_failed
            ));
            self.refresh_proxies().await;
        }

        for msg in bulk_logs {
            self.push_log(msg);
        }

        self.push_traffic_samples();

        if self.tick_count % 25 == 0 {
            if self.probe_mihomo_health().await {
                self.health_probe_failures = 0;
            } else {
                self.health_probe_failures = self.health_probe_failures.saturating_add(1);
                if self.health_probe_failures >= 2 {
                    self.push_log("mihomo health degraded, trying recovery");
                    if self.ensure_mihomo_ready(false).await {
                        self.health_probe_failures = 0;
                        self.init_streams().await;
                        self.refresh_proxies().await;
                    }
                }
            }
        }
    }

    async fn refresh_proxies(&mut self) {
        match self.mihomo.get_proxies().await {
            Ok(p) => {
                self.proxy_groups = extract_proxy_groups(&p);
                self.proxies = Some(p);
                self.sync_proxy_cursor();
                self.push_log("proxy list refreshed");
            }
            Err(err) => self.push_log(format!("refresh proxies failed: {err}")),
        }
    }

    fn sync_proxy_cursor(&mut self) {
        if self.proxy_groups.is_empty() {
            self.selected_group_idx = 0;
            self.selected_proxy_idx = 0;
            return;
        }

        if self.selected_group_idx >= self.proxy_groups.len() {
            self.selected_group_idx = 0;
        }

        let selected = &self.proxy_groups[self.selected_group_idx];
        if selected.candidates.is_empty() {
            self.selected_proxy_idx = 0;
            return;
        }

        if let Some(idx) = selected.candidates.iter().position(|v| v == &selected.now) {
            self.selected_proxy_idx = idx;
            return;
        }

        if self.selected_proxy_idx >= selected.candidates.len() {
            self.selected_proxy_idx = 0;
        }
    }

    fn sync_profile_cursor(&mut self) {
        if self.store.state.profiles.is_empty() {
            self.selected_profile_idx = 0;
            return;
        }
        if self.selected_profile_idx >= self.store.state.profiles.len() {
            self.selected_profile_idx = 0;
        }
    }

    fn move_group_cursor(&mut self, delta: isize) {
        let len = self.proxy_groups.len();
        if len == 0 {
            return;
        }
        self.selected_group_idx = wrap_index(self.selected_group_idx, len, delta);
        self.selected_proxy_idx = 0;
        self.sync_proxy_cursor();
    }

    fn move_candidate_cursor(&mut self, delta: isize) {
        let Some(group) = self.proxy_groups.get(self.selected_group_idx) else {
            return;
        };
        let len = group.candidates.len();
        if len == 0 {
            self.selected_proxy_idx = 0;
            return;
        }
        self.selected_proxy_idx = wrap_index(self.selected_proxy_idx, len, delta);
    }

    fn move_profile_cursor(&mut self, delta: isize) {
        let len = self.store.state.profiles.len();
        if len == 0 {
            self.selected_profile_idx = 0;
            return;
        }
        self.selected_profile_idx = wrap_index(self.selected_profile_idx, len, delta);
    }

    async fn activate_selected_profile(&mut self) {
        let Some(profile) = self.store.state.profiles.get(self.selected_profile_idx) else {
            self.push_log("no profile selected");
            return;
        };
        let uid = profile.uid.clone();
        let name = profile.name.clone();
        let file = profile.file.clone();

        self.store.state.current = Some(uid);
        if let Err(err) = self.store.save().await {
            self.push_log(format!("save current profile failed: {err}"));
            return;
        }
        self.push_log(format!("current profile => {name}"));

        if self.mihomo.get_version().await.is_err() {
            self.push_log("mihomo api unavailable, profile only switched in local state");
            return;
        }

        match self.apply_profile_file_to_core(&file).await {
            Ok(_) => {
                self.push_log("profile config applied to mihomo");
                self.refresh_proxies().await;
            }
            Err(err) => self.push_log(format!("apply profile to mihomo failed: {err}")),
        }
    }

    async fn refresh_profile_subscription_by_uid(&mut self, uid: &str) -> bool {
        let Some(profile) = self.store.state.profiles.iter().find(|p| p.uid == uid).cloned() else {
            self.push_log(format!("profile not found: {uid}"));
            return false;
        };

        if profile.url.trim().is_empty() {
            self.push_log(format!("profile has empty url: {}", profile.name));
            return false;
        }

        let attempts = [
            (
                "direct",
                ImportOptions {
                    timeout_seconds: 20,
                    ..ImportOptions::default()
                },
            ),
            (
                "self_proxy",
                ImportOptions {
                    timeout_seconds: 20,
                    self_proxy: true,
                    ..ImportOptions::default()
                },
            ),
            (
                "system_proxy",
                ImportOptions {
                    timeout_seconds: 20,
                    with_proxy: true,
                    ..ImportOptions::default()
                },
            ),
        ];

        let mut updated = None;
        for (mode, options) in attempts {
            match self.store.update_profile(uid, &options).await {
                Ok(profile) => {
                    self.push_log(format!("subscription updated: {} ({uid}) via {mode}", profile.name));
                    updated = Some(profile);
                    break;
                }
                Err(err) => self.push_log(format!("update {} ({uid}) via {mode} failed: {err}", profile.name)),
            }
        }

        let Some(updated) = updated else {
            self.push_log(format!("subscription update failed: {} ({uid})", profile.name));
            return false;
        };

        if self.store.state.current.as_deref() == Some(uid) {
            if self.ensure_mihomo_ready(false).await {
                match self.apply_profile_file_to_core(&updated.file).await {
                    Ok(_) => {
                        self.push_log(format!("applied updated profile to mihomo: {}", updated.name));
                        self.refresh_proxies().await;
                    }
                    Err(err) => self.push_log(format!("apply updated profile to mihomo failed: {err}")),
                }
            } else {
                self.push_log("mihomo unavailable, updated profile saved locally only");
            }
        }

        true
    }

    async fn refresh_selected_profile_subscription(&mut self) {
        let Some(uid) = self.selected_profile().map(|p| p.uid.clone()) else {
            self.push_log("no profile selected");
            return;
        };
        let _ = self.refresh_profile_subscription_by_uid(&uid).await;
    }

    async fn refresh_all_profile_subscriptions(&mut self) {
        let uids = self
            .store
            .state
            .profiles
            .iter()
            .map(|p| p.uid.clone())
            .collect::<Vec<_>>();
        if uids.is_empty() {
            self.push_log("no profiles to update");
            return;
        }

        let mut ok = 0usize;
        let mut failed = 0usize;
        for uid in uids {
            if self.refresh_profile_subscription_by_uid(&uid).await {
                ok += 1;
            } else {
                failed += 1;
            }
        }
        self.push_log(format!("subscription refresh completed: ok {ok}, failed {failed}"));
    }

    fn selected_profile(&self) -> Option<&verge_core::ProfileItem> {
        self.store.state.profiles.get(self.selected_profile_idx)
    }

    fn selected_group_and_proxy(&self) -> Option<(&str, &str)> {
        let group = self.proxy_groups.get(self.selected_group_idx)?;
        let proxy = group.candidates.get(self.selected_proxy_idx)?;
        Some((group.name.as_str(), proxy.as_str()))
    }

    async fn switch_selected_proxy(&mut self) {
        let Some((group, proxy)) = self.selected_group_and_proxy() else {
            self.push_log("no proxy selection");
            return;
        };

        match self.mihomo.select_node_for_group(group, proxy).await {
            Ok(_) => {
                self.push_log(format!("switched by cursor: {group} -> {proxy}"));
                self.refresh_proxies().await;
            }
            Err(err) => self.push_log(format!("switch failed: {err}")),
        }
    }

    async fn test_selected_proxy_delay(&mut self) {
        let Some((_, proxy)) = self.selected_group_and_proxy() else {
            self.push_log("no proxy selection");
            return;
        };
        let proxy = proxy.to_string();
        let url = self.store.state.verge.default_delay_test_url.clone();
        match self.mihomo.delay_proxy_by_name(&proxy, &url, 5_000).await {
            Ok(resp) => {
                self.delay_cache.insert(proxy.clone(), resp.delay);
                self.push_log(format!("delay {proxy}: {} ms ({url})", resp.delay));
            }
            Err(err) => self.push_log(format!("delay failed for {proxy}: {err}")),
        }
    }

    fn collect_bulk_delay_targets(&self) -> Vec<String> {
        let mut targets = HashSet::new();
        let group_names = self
            .proxy_groups
            .iter()
            .map(|g| g.name.as_str())
            .collect::<HashSet<_>>();

        for group in &self.proxy_groups {
            for candidate in &group.candidates {
                if group_names.contains(candidate.as_str()) {
                    continue;
                }
                if is_builtin_policy_node(candidate) {
                    continue;
                }
                targets.insert(candidate.clone());
            }
        }

        let mut out = targets.into_iter().collect::<Vec<_>>();
        out.sort();
        out
    }

    fn start_bulk_delay_test(&mut self, url: String, timeout_ms: u64) {
        if self.bulk_delay_running {
            self.push_log("bulk delay is already running");
            return;
        }

        let targets = self.collect_bulk_delay_targets();
        if targets.is_empty() {
            self.push_log("no testable nodes found for bulk delay");
            return;
        }

        let total = targets.len();
        let client = self.mihomo.clone();
        let (tx, rx) = mpsc::channel(512);

        self.bulk_delay_rx = Some(rx);
        self.bulk_delay_running = true;
        self.bulk_delay_total = total;
        self.bulk_delay_done = 0;
        self.bulk_delay_success = 0;
        self.bulk_delay_failed = 0;
        self.bulk_delay_url = url.clone();
        self.bulk_delay_timeout_ms = timeout_ms;

        tokio::spawn(async move {
            let _ = tx
                .send(BulkDelayEvent::Started {
                    total,
                    url: url.clone(),
                    timeout_ms,
                })
                .await;

            for node in targets {
                match client.delay_proxy_by_name(&node, &url, timeout_ms).await {
                    Ok(resp) => {
                        let _ = tx
                            .send(BulkDelayEvent::Item {
                                node,
                                delay: Some(resp.delay),
                                error: None,
                            })
                            .await;
                    }
                    Err(err) => {
                        let _ = tx
                            .send(BulkDelayEvent::Item {
                                node,
                                delay: None,
                                error: Some(err.to_string()),
                            })
                            .await;
                    }
                }
            }

            let _ = tx.send(BulkDelayEvent::Finished).await;
        });
    }

    fn push_traffic_samples(&mut self) {
        push_capped(&mut self.traffic_up_history, self.traffic.up, TRAFFIC_HISTORY_LIMIT);
        push_capped(&mut self.traffic_down_history, self.traffic.down, TRAFFIC_HISTORY_LIMIT);
    }

    fn uptime_hms(&self) -> String {
        let secs = self.app_started_at.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }

    async fn apply_tun_mode(&mut self, enabled: bool) -> Result<()> {
        self.push_log(format!(
            "apply tun requested: enable={enabled}, device={TUI_TUN_DEVICE}, stack=gvisor, auto-route=true, dns-hijack=any:53"
        ));
        let patch = if enabled {
            serde_json::json!({
                "tun": {
                    "enable": true,
                    "device": TUI_TUN_DEVICE,
                    "stack": "gvisor",
                    "auto-route": true,
                    "strict-route": false,
                    "auto-detect-interface": true,
                    "dns-hijack": ["any:53"]
                },
                "dns": {
                    "enable": true,
                    "enhanced-mode": "fake-ip",
                    "fake-ip-range": "198.18.0.1/16"
                }
            })
        } else {
            serde_json::json!({
                "tun": { "enable": false }
            })
        };

        self.mihomo
            .patch_base_config(&patch)
            .await
            .context("apply tun config through mihomo api failed")?;

        if let Ok(cfg) = self.mihomo.get_base_config().await {
            let tun_enable = cfg.get("tun").and_then(|v| v.get("enable")).and_then(|v| v.as_bool());
            let tun_stack = cfg
                .get("tun")
                .and_then(|v| v.get("stack"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let tun_device = cfg
                .get("tun")
                .and_then(|v| v.get("device"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            self.push_log(format!(
                "tun post-check: enable={:?}, device={}, stack={}, endpoint={}",
                tun_enable,
                tun_device,
                tun_stack,
                self.mihomo.endpoint_label()
            ));

            if enabled && tun_enable != Some(true) {
                let core_log = self
                    .store
                    .paths
                    .root
                    .join("core-home")
                    .join("logs")
                    .join("managed-core.log");
                let tails = tail_text_lines(&core_log, 16);
                for line in &tails {
                    self.push_log(format!("core-log> {line}"));
                }
                let hint = if tails
                    .iter()
                    .any(|l| l.contains("Start TUN listening error") && l.contains("operation not permitted"))
                {
                    "tun enable rejected by core: operation not permitted (missing NET_ADMIN/root privileges)"
                        .to_string()
                } else {
                    "tun enable rejected by core (tun.enable stayed false)".to_string()
                };
                self.push_log(tun_privilege_hint());
                bail!(hint);
            }
        }
        Ok(())
    }

    fn managed_http_controller_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}",
            self.store.state.verge.mixed_port.saturating_add(1200)
        )
    }

    fn switch_tab_prev(&mut self) {
        if self.tab_index == 0 {
            self.tab_index = Tab::ALL.len() - 1;
        } else {
            self.tab_index -= 1;
        }
    }

    fn switch_tab_next(&mut self) {
        self.tab_index = (self.tab_index + 1) % Tab::ALL.len();
    }

    fn current_tab(&self) -> Tab {
        Tab::ALL[self.tab_index]
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        if self.show_exit_confirm_overlay {
            match key.code {
                KeyCode::Esc => {
                    self.show_exit_confirm_overlay = false;
                }
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    self.exit_confirm_choice = ExitConfirmChoice::KeepBackend;
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    self.exit_confirm_choice = ExitConfirmChoice::StopBackend;
                }
                KeyCode::Enter => {
                    self.confirm_quit_by_choice().await;
                }
                _ => {}
            }
            return Ok(());
        }

        if self.show_help_overlay {
            if matches!(key.code, KeyCode::Esc) {
                self.show_help_overlay = false;
            }
            return Ok(());
        }

        if self.command_mode {
            return self.handle_command_mode_key(key).await;
        }

        if matches!(self.current_tab(), Tab::Profiles) {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_profile_cursor(-1);
                    return Ok(());
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_profile_cursor(1);
                    return Ok(());
                }
                KeyCode::Enter => {
                    self.activate_selected_profile().await;
                    return Ok(());
                }
                KeyCode::Char('u') => {
                    self.refresh_selected_profile_subscription().await;
                    return Ok(());
                }
                _ => {}
            }
        }

        if matches!(self.current_tab(), Tab::Proxies) {
            match self.proxy_focus {
                ProxyFocus::Groups => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.move_group_cursor(-1);
                        return Ok(());
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.move_group_cursor(1);
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        if self
                            .proxy_groups
                            .get(self.selected_group_idx)
                            .is_some_and(|g| !g.candidates.is_empty())
                        {
                            self.proxy_focus = ProxyFocus::Candidates;
                        } else {
                            self.push_log("selected group has no candidates");
                        }
                        return Ok(());
                    }
                    _ => {}
                },
                ProxyFocus::Candidates => match key.code {
                    KeyCode::Left | KeyCode::Esc | KeyCode::Char('h') | KeyCode::Char('j') => {
                        self.proxy_focus = ProxyFocus::Groups;
                        return Ok(());
                    }
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('[') => {
                        self.move_candidate_cursor(-1);
                        return Ok(());
                    }
                    KeyCode::Down | KeyCode::Char(']') => {
                        self.move_candidate_cursor(1);
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        self.switch_selected_proxy().await;
                        return Ok(());
                    }
                    KeyCode::Char('t') => {
                        self.test_selected_proxy_delay().await;
                        return Ok(());
                    }
                    KeyCode::Char('T') => {
                        self.start_bulk_delay_test(self.store.state.verge.default_delay_test_url.clone(), 5_000);
                        return Ok(());
                    }
                    _ => {}
                },
            }
        }

        let lock_tab_switch = matches!(self.current_tab(), Tab::Proxies) && self.proxy_focus == ProxyFocus::Candidates;
        match key.code {
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char(':') => {
                self.command_mode = true;
                self.command_input.clear();
            }
            KeyCode::Char('r') => self.refresh_proxies().await,
            KeyCode::Tab if !lock_tab_switch => self.switch_tab_next(),
            KeyCode::BackTab if !lock_tab_switch => self.switch_tab_prev(),
            KeyCode::Left | KeyCode::Char('h') if !lock_tab_switch => self.switch_tab_prev(),
            KeyCode::Right | KeyCode::Char('l') if !lock_tab_switch => self.switch_tab_next(),
            _ => {}
        }

        Ok(())
    }

    async fn handle_command_mode_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.command_mode = false;
                self.command_input.clear();
            }
            KeyCode::Backspace => {
                self.command_input.pop();
            }
            KeyCode::Enter => {
                let input = self.command_input.trim().to_string();
                self.command_mode = false;
                self.command_input.clear();
                if !input.is_empty() {
                    self.execute_command(&input).await?;
                }
            }
            KeyCode::Char(c) => {
                self.command_input.push(c);
            }
            _ => {}
        }
        Ok(())
    }

    async fn execute_command(&mut self, cmd: &str) -> Result<()> {
        let mut parts = cmd.split_whitespace();
        let Some(action) = parts.next() else {
            return Ok(());
        };

        match action {
            "help" => {
                self.show_help_overlay = true;
            }
            "sysproxy" => {
                match parts.next() {
                    Some("on") => {
                        self.store.state.verge.enable_system_proxy = true;
                        match apply_system_proxy(&self.store.state.verge) {
                            Ok(_) => {
                                self.store.save().await?;
                                self.push_log("system proxy applied => true");
                            }
                            Err(err) => self.push_log(format!("apply system proxy failed: {err}")),
                        }
                    }
                    Some("off") => {
                        self.store.state.verge.enable_system_proxy = false;
                        match apply_system_proxy(&self.store.state.verge) {
                            Ok(_) => {
                                self.store.save().await?;
                                self.push_log("system proxy applied => false");
                            }
                            Err(err) => self.push_log(format!("apply system proxy failed: {err}")),
                        }
                    }
                    Some("toggle") | None => {
                        let original = self.store.state.verge.enable_system_proxy;
                        self.store.state.verge.enable_system_proxy = !original;
                        match apply_system_proxy(&self.store.state.verge) {
                            Ok(_) => {
                                self.store.save().await?;
                                self.push_log(format!(
                                    "system proxy applied => {}",
                                    self.store.state.verge.enable_system_proxy
                                ));
                            }
                            Err(err) => {
                                self.store.state.verge.enable_system_proxy = original;
                                self.push_log(format!("apply system proxy failed: {err}"));
                            }
                        }
                    }
                    _ => self.push_log("usage: sysproxy [on|off|toggle]"),
                }
            }
            "doctor" => {
                self.push_log(format!(
                    "doctor: independent-core={}, service-ipc={}",
                    prefers_independent_core(),
                    prefers_service_ipc()
                ));
                self.push_log(format!("doctor: endpoint={}", self.mihomo.endpoint_label()));
                self.push_log(format!(
                    "doctor: state mixed-port={}, sysproxy={}",
                    self.store.state.verge.mixed_port,
                    self.store.state.verge.enable_system_proxy
                ));
                self.push_log(format!(
                    "doctor: cleanup-on-exit={} backend-exit-policy={} (effective keep={}) {}",
                    self.store.state.verge.auto_cleanup_on_exit,
                    backend_exit_policy_label(self.store.state.verge.backend_exit_policy),
                    self.effective_keep_core_on_exit(),
                    self.auto_update_status_line()
                ));
                if let Some(pid) = self.read_managed_core_pid() {
                    self.push_log(format!(
                        "doctor: managed-core pid={pid}, alive={}",
                        is_pid_alive(pid)
                    ));
                }
                if let Ok(cfg) = self.mihomo.get_base_config().await {
                    let core_mixed = cfg
                        .get("mixed-port")
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u16::try_from(v).ok());
                    self.push_log(format!("doctor: core mixed-port={core_mixed:?}"));
                }
                if let Some(core) = resolve_core_binary(None) {
                    self.push_log(format!("doctor: core-bin={}", core.display()));
                    if let Some(has_caps) = core_has_linux_tun_caps(&core) {
                        self.push_log(format!("doctor: linux tun caps={has_caps}"));
                        if !has_caps {
                            self.push_log(tun_privilege_hint());
                        }
                    }
                } else {
                    self.push_log("doctor: core-bin not found in PATH");
                }
                self.push_log(format!(
                    "doctor: service ipc primary exists={} legacy exists={}",
                    Path::new(SERVICE_IPC_PRIMARY_PATH).exists(),
                    Path::new(SERVICE_IPC_LEGACY_PATH).exists()
                ));
                let _ = self.check_mihomo_health().await;
            }
            "logpath" => {
                if let Some(path) = self.file_log_path.as_ref() {
                    self.push_log(format!("log file => {}", path.display()));
                } else {
                    self.push_log("log file is not available");
                }
                if let Some(path) = self.session_log_path.as_ref() {
                    self.push_log(format!("session log => {}", path.display()));
                }
            }
            "health" => {
                let _ = self.ensure_mihomo_ready(true).await;
            }
            "adopt" => {
                self.try_adopt_clash_verge_controller().await;
                if !self.check_mihomo_health().await {
                    let _ = self.try_start_managed_mihomo().await;
                    let _ = self.check_mihomo_health().await;
                }
            }
            "import" => {
                let Some(url) = parts.next() else {
                    self.push_log("usage: import <url>");
                    return Ok(());
                };

                let attempts = [
                    (
                        "direct",
                        ImportOptions {
                            timeout_seconds: 20,
                            ..ImportOptions::default()
                        },
                    ),
                    (
                        "self_proxy",
                        ImportOptions {
                            timeout_seconds: 20,
                            self_proxy: true,
                            ..ImportOptions::default()
                        },
                    ),
                    (
                        "system_proxy",
                        ImportOptions {
                            timeout_seconds: 20,
                            with_proxy: true,
                            ..ImportOptions::default()
                        },
                    ),
                ];

                let mut imported = false;
                for (mode, options) in attempts {
                    match self.store.import_profile(url, &options).await {
                        Ok(profile) => {
                            self.push_log(format!(
                                "imported profile: {} ({}) via {mode}",
                                profile.name, profile.uid
                            ));
                            self.selected_profile_idx = self.store.state.profiles.len().saturating_sub(1);
                            self.sync_profile_cursor();
                            self.activate_selected_profile().await;
                            imported = true;
                            break;
                        }
                        Err(err) => {
                            self.push_log(format!("import ({mode}) failed: {err}"));
                        }
                    }
                }

                if !imported {
                    self.push_log("import failed after retries");
                }
            }
            "reload" => {
                let target = parts.next().unwrap_or_default();
                if target == "proxies" {
                    self.refresh_proxies().await;
                } else if target == "subscriptions" || target == "profiles" {
                    self.refresh_all_profile_subscriptions().await;
                } else {
                    self.push_log("usage: reload proxies|subscriptions");
                }
            }
            "update" => {
                let target = parts.next().unwrap_or("selected");
                if target == "selected" {
                    self.refresh_selected_profile_subscription().await;
                } else if target == "all" {
                    self.refresh_all_profile_subscriptions().await;
                } else {
                    let _ = self.refresh_profile_subscription_by_uid(target).await;
                }
            }
            "autosub" => match parts.next() {
                Some("now") => {
                    self.push_log("autosub: manual trigger");
                    self.refresh_all_profile_subscriptions().await;
                    self.schedule_next_auto_update("manual");
                }
                Some("status") | None => {
                    self.push_log(self.auto_update_status_line());
                }
                Some("off") | Some("disable") | Some("0") => {
                    self.store.state.verge.auto_update_subscription_minutes = 0;
                    self.store.save().await?;
                    self.schedule_next_auto_update("user");
                }
                Some(v) => {
                    let Ok(minutes) = v.parse::<u64>() else {
                        self.push_log("usage: autosub [off|status|now|<minutes>]");
                        return Ok(());
                    };
                    if minutes == 0 {
                        self.store.state.verge.auto_update_subscription_minutes = 0;
                    } else {
                        self.store.state.verge.auto_update_subscription_minutes = minutes;
                    }
                    self.store.save().await?;
                    self.schedule_next_auto_update("user");
                }
            },
            "switch" => {
                let Some(group) = parts.next() else {
                    self.push_log("usage: switch <group> <proxy>");
                    return Ok(());
                };
                let Some(proxy) = parts.next() else {
                    self.push_log("usage: switch <group> <proxy>");
                    return Ok(());
                };

                match self.mihomo.select_node_for_group(group, proxy).await {
                    Ok(_) => {
                        self.push_log(format!("switched: {group} -> {proxy}"));
                        self.refresh_proxies().await;
                    }
                    Err(err) => self.push_log(format!("switch failed: {err}")),
                }
            }
            "delay" => {
                let Some(proxy) = parts.next() else {
                    self.push_log("usage: delay <proxy|selected|all> [url] [timeout_ms]");
                    return Ok(());
                };

                let url = parts
                    .next()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| self.store.state.verge.default_delay_test_url.clone());
                let timeout_ms = parts
                    .next()
                    .and_then(|x| x.parse::<u64>().ok())
                    .unwrap_or(5_000);

                if proxy == "all" {
                    self.start_bulk_delay_test(url, timeout_ms);
                    return Ok(());
                }

                let proxy_name = if proxy == "selected" {
                    if let Some((_, selected)) = self.selected_group_and_proxy() {
                        selected.to_string()
                    } else {
                        self.push_log("no proxy selection");
                        return Ok(());
                    }
                } else {
                    proxy.to_string()
                };

                match self
                    .mihomo
                    .delay_proxy_by_name(&proxy_name, &url, timeout_ms)
                    .await
                {
                    Ok(resp) => {
                        self.delay_cache.insert(proxy_name.clone(), resp.delay);
                        self.push_log(format!("delay {proxy_name}: {} ms", resp.delay));
                    }
                    Err(err) => self.push_log(format!("delay failed: {err}")),
                }
            }
            "mode" => {
                let Some(mode) = parts.next() else {
                    self.push_log("usage: mode <rule|global|direct>");
                    return Ok(());
                };
                if !matches!(mode, "rule" | "global" | "direct") {
                    self.push_log("mode must be one of: rule, global, direct");
                    return Ok(());
                }

                match self
                    .mihomo
                    .patch_base_config(&serde_json::json!({"mode": mode}))
                    .await
                {
                    Ok(_) => self.push_log(format!("mode changed to {mode}")),
                    Err(err) => self.push_log(format!("change mode failed: {err}")),
                }
            }
            "cleanup" => {
                self.cleanup_before_exit().await;
            }
            "backend" => match parts.next() {
                Some("status") | None => {
                    self.push_log(format!("backend endpoint: {}", self.mihomo.endpoint_label()));
                    self.push_log(format!(
                        "backend-exit-policy={} (effective keep={})",
                        backend_exit_policy_label(self.store.state.verge.backend_exit_policy),
                        self.effective_keep_core_on_exit()
                    ));
                    if let Some(pid) = self.read_managed_core_pid() {
                        self.push_log(format!("managed core pid={pid}, alive={}", is_pid_alive(pid)));
                    } else {
                        self.push_log("managed core pid file: not found");
                    }
                }
                Some("start") => {
                    let _ = self.ensure_mihomo_ready(true).await;
                    self.refresh_proxies().await;
                }
                Some("stop") => {
                    self.cleanup_before_exit().await;
                    self.stop_managed_core_backend().await;
                }
                Some("keep") => {
                    let Some(v) = parts.next() else {
                        self.push_log("usage: backend keep <on|off>");
                        return Ok(());
                    };
                    let Some(keep) = parse_on_off(v) else {
                        self.push_log("usage: backend keep <on|off>");
                        return Ok(());
                    };
                    self.store.state.verge.keep_core_on_exit = keep;
                    self.store.state.verge.backend_exit_policy = if keep {
                        BackendExitPolicy::AlwaysOn
                    } else {
                        BackendExitPolicy::AlwaysOff
                    };
                    self.store.save().await?;
                    self.push_log(format!(
                        "backend-exit-policy => {}",
                        backend_exit_policy_label(self.store.state.verge.backend_exit_policy)
                    ));
                }
                Some("policy") => {
                    let Some(v) = parts.next() else {
                        self.push_log("usage: backend policy <always-on|always-off|query>");
                        return Ok(());
                    };
                    let Some(policy) = parse_backend_exit_policy(v) else {
                        self.push_log("usage: backend policy <always-on|always-off|query>");
                        return Ok(());
                    };
                    self.store.state.verge.backend_exit_policy = policy;
                    if policy != BackendExitPolicy::Query {
                        self.store.state.verge.keep_core_on_exit =
                            matches!(policy, BackendExitPolicy::AlwaysOn);
                    }
                    self.store.save().await?;
                    self.push_log(format!(
                        "backend-exit-policy => {}",
                        backend_exit_policy_label(policy)
                    ));
                }
                _ => {
                    self.push_log("usage: backend [status|start|stop|keep <on|off>|policy <always-on|always-off|query>]")
                }
            },
            "toggle" => {
                match parts.next() {
                    Some("sysproxy") => {
                        let original = self.store.state.verge.enable_system_proxy;
                        self.store.state.verge.enable_system_proxy = !self.store.state.verge.enable_system_proxy;
                        match apply_system_proxy(&self.store.state.verge) {
                            Ok(_) => {
                                self.store.save().await?;
                                self.push_log(format!(
                                    "system proxy applied => {}",
                                    self.store.state.verge.enable_system_proxy
                                ));
                            }
                            Err(err) => {
                                self.store.state.verge.enable_system_proxy = original;
                                self.push_log(format!("apply system proxy failed: {err}"));
                            }
                        }
                    }
                    Some("tun") => {
                        let original = self.store.state.verge.enable_tun_mode;
                        self.store.state.verge.enable_tun_mode = !self.store.state.verge.enable_tun_mode;
                        match self.apply_tun_mode(self.store.state.verge.enable_tun_mode).await {
                            Ok(_) => {
                                self.store.save().await?;
                                self.push_log(format!(
                                    "tun applied => {}",
                                    self.store.state.verge.enable_tun_mode
                                ));
                            }
                            Err(err) => {
                                self.store.state.verge.enable_tun_mode = original;
                                self.push_log(format!("apply tun failed: {err}"));
                            }
                        }
                    }
                    _ => self.push_log("usage: toggle sysproxy|tun"),
                }
            }
            "set" => {
                match parts.next() {
                    Some("controller") => {
                        let Some(url) = parts.next() else {
                            self.push_log("usage: set controller <url>");
                            return Ok(());
                        };
                        self.store.state.verge.controller_url = url.to_string();
                        self.store.save().await?;
                        self.recreate_client().await?;
                        self.push_log(format!("controller set to {url}"));
                        self.refresh_proxies().await;
                    }
                    Some("secret") => {
                        let Some(secret) = parts.next() else {
                            self.push_log("usage: set secret <secret>");
                            return Ok(());
                        };
                        self.store.state.verge.secret = secret.to_string();
                        self.store.save().await?;
                        self.recreate_client().await?;
                        self.push_log("secret updated".to_string());
                        self.refresh_proxies().await;
                    }
                    Some("mixed-port") => {
                        let Some(port) = parts.next().and_then(|v| v.parse::<u16>().ok()) else {
                            self.push_log("usage: set mixed-port <port>");
                            return Ok(());
                        };
                        self.store.state.verge.mixed_port = port;
                        self.store.save().await?;
                        self.push_log(format!("mixed-port set to {port}"));
                        if self.store.state.verge.enable_system_proxy {
                            match apply_system_proxy(&self.store.state.verge) {
                                Ok(_) => self.push_log("system proxy re-applied with new port"),
                                Err(err) => self.push_log(format!("re-apply system proxy failed: {err}")),
                            }
                        }
                    }
                    Some("proxy-host") => {
                        let Some(host) = parts.next() else {
                            self.push_log("usage: set proxy-host <host>");
                            return Ok(());
                        };
                        self.store.state.verge.proxy_host = host.to_string();
                        self.store.save().await?;
                        self.push_log(format!("proxy-host set to {host}"));
                        if self.store.state.verge.enable_system_proxy {
                            match apply_system_proxy(&self.store.state.verge) {
                                Ok(_) => self.push_log("system proxy re-applied with new host"),
                                Err(err) => self.push_log(format!("re-apply system proxy failed: {err}")),
                            }
                        }
                    }
                    Some("cleanup-on-exit") => {
                        let Some(v) = parts.next() else {
                            self.push_log("usage: set cleanup-on-exit <on|off>");
                            return Ok(());
                        };
                        let Some(enabled) = parse_on_off(v) else {
                            self.push_log("usage: set cleanup-on-exit <on|off>");
                            return Ok(());
                        };
                        self.store.state.verge.auto_cleanup_on_exit = enabled;
                        self.store.save().await?;
                        self.push_log(format!("cleanup-on-exit => {enabled}"));
                    }
                    Some("keep-core-on-exit") => {
                        let Some(v) = parts.next() else {
                            self.push_log("usage: set keep-core-on-exit <on|off>");
                            return Ok(());
                        };
                        let Some(enabled) = parse_on_off(v) else {
                            self.push_log("usage: set keep-core-on-exit <on|off>");
                            return Ok(());
                        };
                        self.store.state.verge.keep_core_on_exit = enabled;
                        self.store.state.verge.backend_exit_policy = if enabled {
                            BackendExitPolicy::AlwaysOn
                        } else {
                            BackendExitPolicy::AlwaysOff
                        };
                        self.store.save().await?;
                        self.push_log(format!(
                            "backend-exit-policy => {}",
                            backend_exit_policy_label(self.store.state.verge.backend_exit_policy)
                        ));
                    }
                    Some("backend-exit-policy") => {
                        let Some(v) = parts.next() else {
                            self.push_log("usage: set backend-exit-policy <always-on|always-off|query>");
                            return Ok(());
                        };
                        let Some(policy) = parse_backend_exit_policy(v) else {
                            self.push_log("usage: set backend-exit-policy <always-on|always-off|query>");
                            return Ok(());
                        };
                        self.store.state.verge.backend_exit_policy = policy;
                        if policy != BackendExitPolicy::Query {
                            self.store.state.verge.keep_core_on_exit =
                                matches!(policy, BackendExitPolicy::AlwaysOn);
                        }
                        self.store.save().await?;
                        self.push_log(format!(
                            "backend-exit-policy => {}",
                            backend_exit_policy_label(policy)
                        ));
                    }
                    Some("auto-update") => {
                        let Some(v) = parts.next() else {
                            self.push_log("usage: set auto-update <off|minutes>");
                            return Ok(());
                        };
                        if v.eq_ignore_ascii_case("off") || v == "0" {
                            self.store.state.verge.auto_update_subscription_minutes = 0;
                        } else if let Ok(minutes) = v.parse::<u64>() {
                            self.store.state.verge.auto_update_subscription_minutes = minutes;
                        } else {
                            self.push_log("usage: set auto-update <off|minutes>");
                            return Ok(());
                        }
                        self.store.save().await?;
                        self.schedule_next_auto_update("set");
                    }
                    _ => self.push_log(
                        "usage: set controller <url> | set secret <secret> | set mixed-port <port> | set proxy-host <host> | set cleanup-on-exit <on|off> | set keep-core-on-exit <on|off> | set backend-exit-policy <always-on|always-off|query> | set auto-update <off|minutes>",
                    ),
                }
            }
            "use" => {
                let Some(uid) = parts.next() else {
                    self.push_log("usage: use <profile_uid>");
                    return Ok(());
                };

                if let Some(idx) = self
                    .store
                    .state
                    .profiles
                    .iter()
                    .position(|profile| profile.uid == uid)
                {
                    self.selected_profile_idx = idx;
                    self.activate_selected_profile().await;
                } else {
                    self.push_log(format!("profile not found: {uid}"));
                }
            }
            "save" => {
                self.store.save().await?;
                self.push_log("state saved");
            }
            "quit" | "exit" => self.request_quit(),
            other => self.push_log(format!("unknown command: {other}")),
        }

        Ok(())
    }
}

fn wrap_index(curr: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    if delta >= 0 {
        (curr + delta as usize) % len
    } else {
        (curr + len - ((-delta) as usize % len)) % len
    }
}

fn extract_proxy_groups(proxies: &ProxiesResp) -> Vec<ProxyGroupView> {
    let mut groups = proxies
        .proxies
        .values()
        .filter_map(|node| {
            let all = node.all.as_ref()?;
            Some(ProxyGroupView {
                name: node.name.clone(),
                kind: node.kind.clone(),
                now: node.now.clone().unwrap_or_else(|| "-".to_string()),
                candidates: all.clone(),
            })
        })
        .collect::<Vec<_>>();

    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}

fn push_capped(queue: &mut VecDeque<u64>, value: u64, cap: usize) {
    if queue.len() >= cap {
        let _ = queue.pop_front();
    }
    queue.push_back(value);
}

fn format_bytes(mut bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut idx = 0usize;
    let mut fraction = 0u64;
    while bytes >= 1024 && idx < UNITS.len() - 1 {
        fraction = bytes % 1024;
        bytes /= 1024;
        idx += 1;
    }
    if idx == 0 {
        return format!("{bytes} {}", UNITS[idx]);
    }
    let decimal = (fraction * 10) / 1024;
    format!("{bytes}.{decimal} {}", UNITS[idx])
}

fn calc_ratio(current: u64, max_seen: u64) -> f64 {
    if max_seen == 0 {
        return 0.0;
    }
    let soft_max = max_seen.saturating_mul(12).saturating_div(10).max(1);
    (current as f64 / soft_max as f64).clamp(0.0, 1.0)
}

fn is_builtin_policy_node(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE"
    )
}

fn epoch_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86_400;
    let h = secs / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn panel<'a, T>(title: T, color: Color) -> Block<'a>
where
    T: Into<Line<'a>>,
{
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(title)
}

fn detect_clash_verge_api_hint() -> Option<ClashVergeApiHint> {
    for dir in clash_verge_app_home_candidates() {
        let config_path = dir.join("config.yaml");
        if !config_path.exists() {
            continue;
        }

        let Ok(raw) = std::fs::read_to_string(&config_path) else {
            continue;
        };
        let Ok(map) = serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(&raw) else {
            continue;
        };

        let controller_url = map
            .get("external-controller")
            .and_then(|v| v.as_str())
            .and_then(normalize_controller_url);

        let socket_path = map
            .get("external-controller-unix")
            .and_then(|v| v.as_str())
            .or_else(|| map.get("external-controller-pipe").and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(std::string::ToString::to_string);

        let secret = map.get("secret").and_then(yaml_string_like);
        let mixed_port = map.get("mixed-port").and_then(yaml_to_u16);

        let verge_path = dir.join("verge.yaml");
        let (enable_external_controller, clash_core) = if verge_path.exists() {
            std::fs::read_to_string(&verge_path)
                .ok()
                .and_then(|text| serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(&text).ok())
                .map(|verge| {
                    let enable = verge
                        .get("enable_external_controller")
                        .and_then(|v| v.as_bool())
                        .or_else(|| verge.get("enable-external-controller").and_then(|v| v.as_bool()));
                    let clash_core = verge
                        .get("clash_core")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(std::string::ToString::to_string);
                    (enable, clash_core)
                })
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        return Some(ClashVergeApiHint {
            controller_url,
            socket_path,
            secret,
            mixed_port,
            enable_external_controller,
            clash_core,
            app_home: dir.clone(),
            source_config: config_path,
        });
    }
    None
}

fn local_socket_candidates(hint: &ClashVergeApiHint) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(path) = hint.socket_path.as_deref() {
        out.push(path.to_string());
    }
    out.push("/tmp/verge/verge-mihomo.sock".to_string());
    out.push("/var/tmp/verge/verge-mihomo.sock".to_string());
    if let Some(parent) = hint.source_config.parent() {
        out.push(
            parent
                .join("verge")
                .join("verge-mihomo.sock")
                .to_string_lossy()
                .to_string(),
        );
    }
    out.sort();
    out.dedup();
    out
}

fn pick_socket_for_spawn(hint: Option<&ClashVergeApiHint>, independent: bool) -> String {
    if !independent
        && let Some(path) = hint.and_then(|h| h.socket_path.as_deref())
        && !path.is_empty()
    {
        return path.to_string();
    }
    #[cfg(unix)]
    {
        "/tmp/verge-tui/verge-mihomo.sock".to_string()
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\verge-tui-mihomo".to_string()
    }
}

fn resolve_core_binary(preferred: Option<&str>) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("VERGE_TUI_CORE_BIN") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    let mut names = Vec::new();
    if let Some(name) = preferred
        && !name.trim().is_empty()
    {
        names.push(name.trim().to_string());
    }
    if !names.iter().any(|n| n == "verge-mihomo") {
        names.push("verge-mihomo".to_string());
    }
    if !names.iter().any(|n| n == "verge-mihomo-alpha") {
        names.push("verge-mihomo-alpha".to_string());
    }

    for name in names {
        let candidate = PathBuf::from(&name);
        if candidate.components().count() > 1 && candidate.exists() {
            return Some(candidate);
        }
        if let Some(found) = find_executable_in_path(&name) {
            return Some(found);
        }
    }
    None
}

fn resolve_service_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("VERGE_TUI_SERVICE_BIN") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    let candidates = ["/usr/bin/clash-verge-service", "clash-verge-service"];
    for candidate in candidates {
        let path = PathBuf::from(candidate);
        if path.components().count() > 1 && path.exists() {
            return Some(path);
        }
        if let Some(found) = find_executable_in_path(candidate) {
            return Some(found);
        }
    }
    None
}

fn tun_privilege_hint() -> String {
    #[cfg(target_os = "linux")]
    {
        let core = resolve_core_binary(None)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/usr/bin/verge-mihomo".to_string());
        return format!(
            "tun permission hint: run `sudo setcap cap_net_admin,cap_net_raw+ep {core}` or start privileged service `sudo systemctl restart clash-verge-service.service`"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        "tun permission hint: run TUI with elevated privileges or use privileged service mode".to_string()
    }
}

fn core_has_linux_tun_caps(core_bin: &Path) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("getcap")
            .arg(core_bin)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        if text.trim().is_empty() {
            return Some(false);
        }
        return Some(text.contains("cap_net_admin") && text.contains("cap_net_raw"));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = core_bin;
        None
    }
}

fn ensure_tun_defaults_in_mapping(map: &mut serde_yaml_ng::Mapping, enable: bool) {
    let tun_key = serde_yaml_ng::Value::from("tun");
    let mut tun = map
        .get(&tun_key)
        .and_then(|v| v.as_mapping().cloned())
        .unwrap_or_default();

    tun.insert("enable".into(), enable.into());
    if !tun.contains_key("stack") {
        tun.insert("stack".into(), "gvisor".into());
    }
    if !tun.contains_key("device") {
        tun.insert("device".into(), TUI_TUN_DEVICE.into());
    }
    if !tun.contains_key("auto-route") {
        tun.insert("auto-route".into(), true.into());
    }
    if !tun.contains_key("strict-route") {
        tun.insert("strict-route".into(), false.into());
    }
    if !tun.contains_key("auto-detect-interface") {
        tun.insert("auto-detect-interface".into(), true.into());
    }
    if !tun.contains_key("dns-hijack") {
        tun.insert(
            "dns-hijack".into(),
            serde_yaml_ng::Value::Sequence(vec!["any:53".into()]),
        );
    }
    map.insert(tun_key, tun.into());
}

fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    #[cfg(target_os = "windows")]
    let exts = [".exe", ".bat", ".cmd", ""];
    #[cfg(not(target_os = "windows"))]
    let exts = [""];

    for dir in std::env::split_paths(&path_env) {
        for ext in &exts {
            let file = if ext.is_empty() {
                dir.join(name)
            } else {
                dir.join(format!("{name}{ext}"))
            };
            if file.exists() {
                return Some(file);
            }
        }
    }
    None
}

fn run_command_probe(cmd: &str, args: &[&str]) -> (bool, String) {
    match std::process::Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(out) => {
            let ok = out.status.success();
            let code = out.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let text = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                "no output".to_string()
            };
            (ok, format!("{cmd} {:?} => ok={ok}, code={code}, msg={text}", args))
        }
        Err(err) => (false, format!("{cmd} {:?} => spawn failed: {err}", args)),
    }
}

fn parse_on_off(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => Some(true),
        "off" | "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn parse_backend_exit_policy(value: &str) -> Option<BackendExitPolicy> {
    match value.trim().to_ascii_lowercase().as_str() {
        "always-on" | "always_on" | "on" => Some(BackendExitPolicy::AlwaysOn),
        "always-off" | "always_off" | "off" => Some(BackendExitPolicy::AlwaysOff),
        "query" | "ask" => Some(BackendExitPolicy::Query),
        _ => None,
    }
}

fn backend_exit_policy_label(policy: BackendExitPolicy) -> &'static str {
    match policy {
        BackendExitPolicy::AlwaysOn => "always-on",
        BackendExitPolicy::AlwaysOff => "always-off",
        BackendExitPolicy::Query => "query",
    }
}

fn terminate_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(unix)]
    {
        if !is_pid_alive(pid) {
            return true;
        }

        let term = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .is_some_and(|s| s.success());

        if !term {
            return false;
        }

        for _ in 0..20 {
            if !is_pid_alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(60));
        }

        std::process::Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .is_some_and(|s| s.success())
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

fn prefers_independent_core() -> bool {
    std::env::var("VERGE_TUI_INDEPENDENT")
        .map(|v| !matches!(v.trim(), "0" | "false" | "False" | "FALSE"))
        .unwrap_or(true)
}

fn prefers_service_ipc() -> bool {
    std::env::var("VERGE_TUI_USE_SERVICE_IPC")
        .map(|v| matches!(v.trim(), "1" | "true" | "True" | "TRUE"))
        .unwrap_or(false)
}

fn pick_runtime_mixed_port(preferred: u16) -> u16 {
    if is_local_port_free(preferred) {
        return preferred;
    }
    for p in MANAGED_PORT_FALLBACK_BASE..(MANAGED_PORT_FALLBACK_BASE + 600) {
        if is_local_port_free(p) {
            return p;
        }
    }
    preferred
}

fn is_local_port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn tail_text_lines(path: &Path, max_lines: usize) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines = content.lines().map(|s| s.trim().to_string()).collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].to_vec()
}

fn clash_verge_app_home_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
        roots.push(PathBuf::from(xdg_data_home));
    }
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(home).join(".local").join("share"));
    }

    let mut candidates = Vec::new();
    let app_ids = [
        "io.github.clash-verge-rev.clash-verge-rev",
        "io.github.clash-verge-rev.clash-verge-rev.dev",
    ];
    for root in roots {
        for app_id in app_ids {
            let dir = root.join(app_id);
            if !candidates.contains(&dir) {
                candidates.push(dir);
            }
        }
    }
    candidates
}

fn normalize_controller_url(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        return Some(value.to_string());
    }
    if value.starts_with(':') {
        return Some(format!("http://127.0.0.1{value}"));
    }
    Some(format!("http://{value}"))
}

fn yaml_string_like(v: &serde_yaml_ng::Value) -> Option<String> {
    match v {
        serde_yaml_ng::Value::String(s) => Some(s.to_string()),
        serde_yaml_ng::Value::Bool(b) => Some(b.to_string()),
        serde_yaml_ng::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn yaml_to_u16(v: &serde_yaml_ng::Value) -> Option<u16> {
    match v {
        serde_yaml_ng::Value::String(s) => s.trim().parse::<u16>().ok(),
        serde_yaml_ng::Value::Number(n) => n.as_u64().map(|x| x as u16),
        _ => None,
    }
}

fn centered_rect(area: ratatui::layout::Rect, percent_x: u16, percent_y: u16) -> ratatui::layout::Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(v[1]);
    h[1]
}

fn draw_help_overlay(frame: &mut ratatui::Frame<'_>, _app: &App) {
    let popup = centered_rect(frame.area(), 90, 84);
    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from("General: q quit | Tab/Shift+Tab or h/l switch tabs | : command mode"),
        Line::from(""),
        Line::from("Profiles: j/k select profile | Enter apply profile | u refresh subscription"),
        Line::from("Proxies (Groups focus): j/k select group | Enter to candidates"),
        Line::from("Proxies (Candidates focus): [/] or Up/Down/k move | Enter switch node"),
        Line::from("Proxies (Candidates focus): t test selected | T test all"),
        Line::from("Proxies (Candidates focus): Left/h/j/Esc back to groups"),
        Line::from(""),
        Line::from("Commands:"),
        Line::from("help | doctor | logpath | health | adopt"),
        Line::from("autosub [off|status|now|<minutes>]"),
        Line::from("backend [status|start|stop|keep <on|off>|policy <always-on|always-off|query>]"),
        Line::from("cleanup"),
        Line::from("sysproxy [on|off|toggle]"),
        Line::from("import <url>"),
        Line::from("reload proxies|subscriptions"),
        Line::from("update [selected|all|<profile_uid>]"),
        Line::from("switch <group> <proxy>"),
        Line::from("delay <proxy|selected|all> [url] [timeout_ms]"),
        Line::from("mode <rule|global|direct>"),
        Line::from("toggle sysproxy|tun"),
        Line::from("set controller <url> | set secret <secret>"),
        Line::from("set mixed-port <port> | set proxy-host <host>"),
        Line::from("set cleanup-on-exit <on|off> | set keep-core-on-exit <on|off>"),
        Line::from("set backend-exit-policy <always-on|always-off|query>"),
        Line::from("set auto-update <off|minutes>"),
        Line::from("use <profile_uid> | save | quit"),
        Line::from(""),
        Line::from("Env: VERGE_TUI_USE_SERVICE_IPC=1 enables service IPC path"),
        Line::from("Press Esc to close this help"),
    ];

    let panel = Paragraph::new(lines)
        .block(panel("Help", COLOR_ACCENT).style(Style::default().bg(COLOR_BG)))
        .style(Style::default().fg(COLOR_TEXT))
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, popup);
}

fn draw_exit_confirm_overlay(frame: &mut ratatui::Frame<'_>, app: &App) {
    let popup = centered_rect(frame.area(), 68, 38);
    frame.render_widget(Clear, popup);

    let keep_selected = matches!(app.exit_confirm_choice, ExitConfirmChoice::KeepBackend);
    let keep_line = if keep_selected {
        Line::from("[*] Keep Backend Running").style(
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Line::from("[ ] Keep Backend Running").style(Style::default().fg(COLOR_TEXT))
    };
    let stop_line = if keep_selected {
        Line::from("[ ] Stop Backend And Cleanup").style(Style::default().fg(COLOR_TEXT))
    } else {
        Line::from("[*] Stop Backend And Cleanup").style(
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_WARN)
                .add_modifier(Modifier::BOLD),
        )
    };

    let lines = vec![
        Line::from("Exit verge-tui").style(Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::from(format!(
            "Current policy: {}",
            backend_exit_policy_label(app.store.state.verge.backend_exit_policy)
        )),
        Line::from("Choose what to do with backend when leaving UI:"),
        Line::from(""),
        keep_line,
        stop_line,
        Line::from(""),
        Line::from("Enter: confirm | Esc: cancel | ←/→ or h/l/j/k: switch"),
    ];

    let panel = Paragraph::new(lines)
        .block(panel("Exit Confirm", COLOR_HOT).style(Style::default().bg(COLOR_BG)))
        .style(Style::default().fg(COLOR_TEXT))
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, popup);
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    let bg = Block::default().style(Style::default().bg(COLOR_BG).fg(COLOR_TEXT));
    frame.render_widget(bg, frame.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(12),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let current_profile = app.store.state.current.as_deref().unwrap_or("-");
    let status_line = Line::from(format!(
        " VERGE-TUI  uptime {}  ticks {}  profile {}  ",
        app.uptime_hms(),
        app.tick_count,
        current_profile
    ))
    .style(Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD));
    let tabs_line = Line::from(
        Tab::ALL
            .iter()
            .enumerate()
            .map(|(idx, tab)| {
                if idx == app.tab_index {
                    format!("[{}]", tab.title())
                } else {
                    tab.title().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("  "),
    );
    let header = Paragraph::new(vec![
        status_line,
        {
            let endpoint = app.mihomo.endpoint_label();
            Line::from(format!(
                " SYS:{}  TUN:{}  Controller:{} ",
                if app.store.state.verge.enable_system_proxy {
                    "ON"
                } else {
                    "OFF"
                },
                if app.store.state.verge.enable_tun_mode {
                    "ON"
                } else {
                    "OFF"
                },
                endpoint
            ))
            .style(Style::default().fg(COLOR_ACCENT))
        },
        tabs_line,
    ])
    .block(panel("Dashboard", COLOR_ACCENT))
    .wrap(Wrap { trim: true })
    .style(Style::default().fg(COLOR_TEXT).bg(COLOR_BG));
    frame.render_widget(header, chunks[0]);

    match app.current_tab() {
        Tab::Overview => draw_overview(frame, chunks[1], app),
        Tab::Profiles => draw_profiles(frame, chunks[1], app),
        Tab::Proxies => draw_proxies(frame, chunks[1], app),
        Tab::Logs => draw_logs(frame, chunks[1], app),
    }

    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(chunks[2]);

    let log_items = app
        .logs
        .iter()
        .rev()
        .take(6)
        .rev()
        .map(|log| ListItem::new(log.clone()))
        .collect::<Vec<_>>();
    let log_widget =
        List::new(log_items).block(panel("Event Feed", COLOR_WARN).style(Style::default().bg(COLOR_BG).fg(COLOR_TEXT)));
    frame.render_widget(log_widget, bottom_chunks[0]);

    let hints = match app.current_tab() {
        Tab::Overview => "h/l,Tab: switch tab\nr: refresh proxies\n:: command mode\nq: quit",
        Tab::Profiles => "j/k: move profile\nEnter: set current\nu: refresh subscription\nuse <uid>\n:help",
        Tab::Proxies => {
            if app.proxy_focus == ProxyFocus::Groups {
                "j/k: select group\nEnter: focus candidates\nTab/h/l: switch tab\n:help"
            } else {
                "[/],↑/↓/k: move node\nEnter: switch  t/T: delay\n←/h/j/Esc: back groups\n(tab locked)"
            }
        }
        Tab::Logs => "Scroll with terminal\n:save persist state\nq: quit",
    };
    let hint_widget = Paragraph::new(hints)
        .block(panel("Keys", COLOR_ACCENT).style(Style::default().bg(COLOR_BG)))
        .style(Style::default().fg(COLOR_TEXT));
    frame.render_widget(hint_widget, bottom_chunks[1]);

    let cmd_text = if app.command_mode {
        format!(":{}", app.command_input)
    } else {
        "Press ':' for commands".to_string()
    };

    let cmd = Paragraph::new(cmd_text)
        .block(
            panel("Command", if app.command_mode { Color::Magenta } else { COLOR_PANEL })
                .style(Style::default().bg(COLOR_BG)),
        )
        .style(Style::default().fg(COLOR_TEXT))
        .wrap(Wrap { trim: true });
    frame.render_widget(cmd, chunks[3]);

    if app.show_help_overlay {
        draw_help_overlay(frame, app);
    }
    if app.show_exit_confirm_overlay {
        draw_exit_confirm_overlay(frame, app);
    }
}

fn draw_overview(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(chunks[0]);

    let current_profile_name = app
        .store
        .state
        .profiles
        .iter()
        .find(|p| app.store.state.current.as_deref() == Some(p.uid.as_str()))
        .map(|p| p.name.as_str())
        .unwrap_or("-");

    let left_lines = vec![
        Line::from(format!("Current Profile : {}", current_profile_name)),
        Line::from(format!("Profiles Count  : {}", app.store.state.profiles.len())),
        Line::from(format!("Proxy Groups    : {}", app.proxy_groups.len())),
        Line::from(format!(
            "Proxy Endpoint  : {}:{}",
            app.store.state.verge.proxy_host, app.store.state.verge.mixed_port
        )),
        Line::from(format!(
            "System Proxy    : {}",
            if app.store.state.verge.enable_system_proxy {
                "ENABLED"
            } else {
                "DISABLED"
            }
        )),
        Line::from(format!(
            "TUN Mode        : {}",
            if app.store.state.verge.enable_tun_mode {
                "ENABLED"
            } else {
                "DISABLED"
            }
        )),
        Line::from(format!(
            "Auto Update     : {}",
            if app.store.state.verge.auto_update_subscription_minutes == 0 {
                "DISABLED".to_string()
            } else {
                format!("{} min", app.store.state.verge.auto_update_subscription_minutes)
            }
        )),
        Line::from(format!(
            "Exit Policy     : cleanup={} backend={} (effective keep={})",
            app.store.state.verge.auto_cleanup_on_exit,
            backend_exit_policy_label(app.store.state.verge.backend_exit_policy),
            app.effective_keep_core_on_exit()
        )),
    ];
    let left_widget =
        Paragraph::new(left_lines).block(panel("Core State", COLOR_ACCENT).style(Style::default().bg(COLOR_BG)));
    frame.render_widget(left_widget, top[0]);

    let up_max = app.traffic_up_history.iter().copied().max().unwrap_or(1);
    let down_max = app.traffic_down_history.iter().copied().max().unwrap_or(1);
    let traffic_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(4)])
        .split(top[1]);

    let up_gauge = Gauge::default()
        .block(panel(format!("Upload {}", format_bytes(app.traffic.up)), COLOR_HOT))
        .gauge_style(Style::default().fg(COLOR_HOT).bg(COLOR_BG))
        .ratio(calc_ratio(app.traffic.up, up_max));
    frame.render_widget(up_gauge, traffic_chunks[0]);

    let down_gauge = Gauge::default()
        .block(panel(
            format!("Download {}", format_bytes(app.traffic.down)),
            COLOR_ACCENT,
        ))
        .gauge_style(Style::default().fg(COLOR_ACCENT).bg(COLOR_BG))
        .ratio(calc_ratio(app.traffic.down, down_max));
    frame.render_widget(down_gauge, traffic_chunks[1]);

    let graph_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(traffic_chunks[2]);
    let up_data = app.traffic_up_history.iter().copied().collect::<Vec<_>>();
    let down_data = app.traffic_down_history.iter().copied().collect::<Vec<_>>();

    let up_graph = Sparkline::default()
        .block(panel("Up History", COLOR_HOT))
        .style(Style::default().fg(COLOR_HOT))
        .data(&up_data);
    frame.render_widget(up_graph, graph_chunks[0]);

    let down_graph = Sparkline::default()
        .block(panel("Down History", COLOR_ACCENT))
        .style(Style::default().fg(COLOR_ACCENT))
        .data(&down_data);
    frame.render_widget(down_graph, graph_chunks[1]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    let profile_items = app
        .store
        .state
        .profiles
        .iter()
        .take(8)
        .map(|profile| {
            let current = if app.store.state.current.as_deref() == Some(profile.uid.as_str()) {
                "*"
            } else {
                " "
            };
            ListItem::new(format!("{current} {}", profile.name))
        })
        .collect::<Vec<_>>();
    let profiles_widget =
        List::new(profile_items).block(panel("Profiles Snapshot", COLOR_PANEL).style(Style::default().bg(COLOR_BG)));
    frame.render_widget(profiles_widget, bottom[0]);

    let network_lines = vec![
        Line::from(format!(
            "Upload Total   : {}",
            format_bytes(app.connections.upload_total)
        )),
        Line::from(format!(
            "Download Total : {}",
            format_bytes(app.connections.download_total)
        )),
        Line::from(format!("Tick Rate      : 200ms")),
        Line::from(format!("Rendered Ticks : {}", app.tick_count)),
    ];
    let net_widget =
        Paragraph::new(network_lines).block(panel("Session Stats", COLOR_PANEL).style(Style::default().bg(COLOR_BG)));
    frame.render_widget(net_widget, bottom[1]);
}

fn draw_profiles(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(area);

    let items = app
        .store
        .state
        .profiles
        .iter()
        .map(|p| {
            let selected = app.store.state.current.as_deref() == Some(p.uid.as_str());
            let marker = if selected { "*" } else { " " };
            let usage = p
                .extra
                .as_ref()
                .map(|e| format!(" usage={}/{}", e.upload + e.download, e.total))
                .unwrap_or_default();
            ListItem::new(format!("{marker} [{}] {}{}", p.uid, p.name, usage))
        })
        .collect::<Vec<_>>();

    let list = List::new(items)
        .block(panel("Profiles (j/k, Enter use)", COLOR_ACCENT).style(Style::default().bg(COLOR_BG)))
        .highlight_style(Style::default().fg(Color::Black).bg(COLOR_ACCENT))
        .highlight_symbol("> ");
    let mut profile_state = ListState::default();
    if !app.store.state.profiles.is_empty() {
        profile_state.select(Some(app.selected_profile_idx));
    }
    frame.render_stateful_widget(list, chunks[0], &mut profile_state);

    let detail_lines = if let Some(profile) = app.selected_profile() {
        let extra = profile.extra.as_ref();
        vec![
            Line::from(format!("Name       : {}", profile.name)),
            Line::from(format!("UID        : {}", profile.uid)),
            Line::from(format!(
                "Current    : {}",
                if app.store.state.current.as_deref() == Some(profile.uid.as_str()) {
                    "YES"
                } else {
                    "NO"
                }
            )),
            Line::from(format!("Updated    : {}", profile.updated)),
            Line::from(format!("URL        : {}", profile.url)),
            Line::from(format!(
                "Usage      : {}/{}",
                extra
                    .map(|e| format_bytes(e.upload + e.download))
                    .unwrap_or_else(|| "-".to_string()),
                extra.map(|e| format_bytes(e.total)).unwrap_or_else(|| "-".to_string())
            )),
            Line::from(format!(
                "Expire     : {}",
                extra.map(|e| e.expire.to_string()).unwrap_or_else(|| "-".to_string())
            )),
        ]
    } else {
        vec![Line::from("No profile loaded")]
    };
    let detail = Paragraph::new(detail_lines)
        .block(panel("Profile Detail", COLOR_PANEL).style(Style::default().bg(COLOR_BG)))
        .wrap(Wrap { trim: true });
    frame.render_widget(detail, chunks[1]);
}

fn draw_proxies(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(8)])
        .split(chunks[0]);

    let group_items = app
        .proxy_groups
        .iter()
        .map(|group| ListItem::new(format!("[{}] {} -> {}", group.kind, group.name, group.now)))
        .collect::<Vec<_>>();
    let groups_title = if app.proxy_focus == ProxyFocus::Groups {
        "Groups [Focus] (j/k, Enter)"
    } else {
        "Groups (left/h/j back)"
    };
    let groups = List::new(group_items)
        .block(panel(groups_title, COLOR_ACCENT).style(Style::default().bg(COLOR_BG)))
        .highlight_style(if app.proxy_focus == ProxyFocus::Groups {
            Style::default().fg(Color::Black).bg(COLOR_ACCENT)
        } else {
            Style::default().fg(COLOR_TEXT).bg(COLOR_PANEL)
        })
        .highlight_symbol("> ");
    let mut groups_state = ListState::default();
    if !app.proxy_groups.is_empty() {
        groups_state.select(Some(app.selected_group_idx));
    }
    frame.render_stateful_widget(groups, left_chunks[0], &mut groups_state);

    let mut candidate_items = Vec::new();
    if let Some(group) = app.proxy_groups.get(app.selected_group_idx) {
        for candidate in &group.candidates {
            let active_marker = if candidate == &group.now { "*" } else { " " };
            let delay = app
                .delay_cache
                .get(candidate)
                .map(|d| format!("{d:>4}ms"))
                .unwrap_or_else(|| "   - ".to_string());
            let item = ListItem::new(format!("{active_marker} {candidate:<36} {delay}"));
            candidate_items.push(item);
        }
    } else {
        candidate_items.push(ListItem::new("No proxy group"));
    }
    let candidates_title = if app.proxy_focus == ProxyFocus::Candidates {
        "Candidates [Focus] ([/], Enter, t/T)"
    } else {
        "Candidates (select group then Enter)"
    };
    let candidates = List::new(candidate_items)
        .block(panel(candidates_title, COLOR_WARN).style(Style::default().bg(COLOR_BG)))
        .highlight_style(if app.proxy_focus == ProxyFocus::Candidates {
            Style::default().fg(Color::Black).bg(COLOR_WARN)
        } else {
            Style::default().fg(COLOR_TEXT).bg(COLOR_PANEL)
        })
        .highlight_symbol("> ");
    let mut candidates_state = ListState::default();
    if app
        .proxy_groups
        .get(app.selected_group_idx)
        .is_some_and(|g| !g.candidates.is_empty())
    {
        candidates_state.select(Some(app.selected_proxy_idx));
    }
    frame.render_stateful_widget(candidates, chunks[1], &mut candidates_state);

    let detail_lines = if let Some(group) = app.proxy_groups.get(app.selected_group_idx) {
        let selected = group
            .candidates
            .get(app.selected_proxy_idx)
            .map(|s| s.as_str())
            .unwrap_or("-");
        let selected_delay = app
            .delay_cache
            .get(selected)
            .map(|d| format!("{d} ms"))
            .unwrap_or_else(|| "-".to_string());
        let bulk_status = if app.bulk_delay_running {
            format!("RUNNING {}/{}", app.bulk_delay_done, app.bulk_delay_total)
        } else if app.bulk_delay_total > 0 {
            format!("DONE {}/{}", app.bulk_delay_success, app.bulk_delay_total)
        } else {
            "IDLE".to_string()
        };
        vec![
            Line::from(format!("Group   : {}", group.name)),
            Line::from(format!("Type    : {}", group.kind)),
            Line::from(format!("Now     : {}", group.now)),
            Line::from(format!("Select  : {}", selected)),
            Line::from(format!("Delay   : {}", selected_delay)),
            Line::from(format!("Choices : {}", group.candidates.len())),
            Line::from(format!(
                "Bulk    : {} (ok {}, fail {})",
                bulk_status, app.bulk_delay_success, app.bulk_delay_failed
            )),
        ]
    } else {
        vec![Line::from("No proxy group selected")]
    };
    let details =
        Paragraph::new(detail_lines).block(panel("Selection Detail", COLOR_PANEL).style(Style::default().bg(COLOR_BG)));
    frame.render_widget(details, left_chunks[1]);
}

fn draw_logs(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(74), Constraint::Percentage(26)])
        .split(area);

    let items = app
        .logs
        .iter()
        .rev()
        .map(|log| {
            let style = if log.contains("failed") || log.contains("error") {
                Style::default().fg(COLOR_HOT)
            } else if log.contains("imported") || log.contains("switched") || log.contains("applied") {
                Style::default().fg(COLOR_ACCENT)
            } else {
                Style::default().fg(COLOR_TEXT)
            };
            ListItem::new(log.clone()).style(style)
        })
        .collect::<Vec<_>>();

    let list = List::new(items).block(panel("Logs", COLOR_WARN).style(Style::default().bg(COLOR_BG)));
    frame.render_widget(list, chunks[0]);

    let panel = Paragraph::new(vec![
        Line::from(format!("Entries: {}", app.logs.len())),
        Line::from(format!("Uptime : {}", app.uptime_hms())),
        Line::from(""),
        Line::from("Use ':' to run"),
        Line::from("commands"),
        Line::from("help/save/quit"),
        Line::from("delay selected/all"),
    ])
    .block(panel("Log Info", COLOR_PANEL).style(Style::default().bg(COLOR_BG)));
    frame.render_widget(panel, chunks[1]);
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut app = App::new().await?;

    enable_raw_mode().context("failed to enable raw mode")?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal")?;

    let tick_rate = Duration::from_millis(200);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &app)).context("render failed")?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_millis(0));

        if event::poll(timeout).context("event poll failed")?
            && let Event::Key(key) = event::read().context("event read failed")?
        {
            app.handle_key(key).await?;
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick().await;
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    app.shutdown().await;

    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to show cursor")?;

    Ok(())
}
