# verge-tui

A terminal-first Clash/Mihomo controller for `clash-verge-rev` workspace.

## Run

```bash
cargo run -p verge-tui
```

Use `VERGE_TUI_HOME` to override state directory (default: `~/.config/verge-tui`).

```bash
VERGE_TUI_HOME=/tmp/verge-tui cargo run -p verge-tui
```

If `verge-mihomo` is not in `PATH`, set core executable manually:

```bash
VERGE_TUI_CORE_BIN=/path/to/verge-mihomo cargo run -p verge-tui
```

## Key Bindings

- `q`: quit
- `Tab` / `Shift+Tab` or `h/l`: switch tabs (only when Proxies focus is on Groups)
- `r`: refresh proxies
- `:`: enter command mode
- `Esc`: exit command mode / close help overlay
- `j/k` (in Proxies tab, Groups focus): select proxy group
- `Enter` (in Proxies tab, Groups focus): switch to Candidates focus
- `[` / `]` or `Up/Down` or `k` (in Proxies tab, Candidates focus): select candidate node
- `Enter` (in Proxies tab, Candidates focus): switch selected node
- `t` (in Proxies tab, Candidates focus): test delay for selected candidate node
- `T` (in Proxies tab, Candidates focus): bulk delay test for all detected nodes
- `Left` / `h` / `j` / `Esc` (in Proxies tab, Candidates focus): back to Groups focus
- `j/k` (in Profiles tab): select profile
- `Enter` (in Profiles tab): set current profile
- `u` (in Profiles tab): refresh selected subscription

## Commands

- `help` (open help overlay, close with `Esc`)
- `health`
- `adopt` (re-detect and adopt Clash Verge controller/socket)
- `import <url>`
- `reload proxies|subscriptions`
- `update [selected|all|<profile_uid>]`
- `switch <group> <proxy>`
- `delay <proxy|selected|all> [url] [timeout_ms]`
- `mode <rule|global|direct>`
- `toggle sysproxy|tun`
- `set controller <url>`
- `set secret <secret>`
- `set mixed-port <port>`
- `set proxy-host <host>`
- `use <profile_uid>`
- `save`
- `quit`

## Notes

- `toggle sysproxy` applies system proxy using `sysproxy-rs`.
- `toggle tun` applies runtime tun patch through Mihomo API.
- `import` and profile activation (`Enter` in Profiles or `use <uid>`) will try to apply
  the selected profile to Mihomo via `PATCH /configs` with `path` + `force=true`.
- On startup, if controller is unreachable, `verge-tui` will try to auto-detect
  Clash Verge's `config.yaml` (`external-controller`, `secret`, `mixed-port`).
- If controller/socket is still unreachable, `verge-tui` will auto-start a managed
  `verge-mihomo` process using Clash Verge runtime config (`clash-verge.yaml`) when available.
- If no Clash Verge runtime config is found, `verge-tui` can generate an independent runtime
  config from the selected subscription profile (`VERGE_TUI_HOME/core-home/verge-tui-runtime.yaml`).
- On Linux/macOS, `verge-tui` prefers starting core through `clash-verge-service` IPC first
  (if available) to keep TUN privilege behavior stable, then falls back to self-managed core.
- While running, `verge-tui` performs periodic health checks and will try auto-recovery
  (re-adopt or restart managed core) if Mihomo becomes unavailable.
