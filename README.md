# verge-tui

`verge-tui` 是一个面向终端的 Mihomo/Clash 控制面，目标是：

- 不依赖桌面环境
- 以 TUI 方式提供核心代理功能
- 支持独立运行（Direct Core First）

本项目从 `clash-verge-rev` 的相关能力中抽离并重构，专注于终端使用场景。

## 功能概览

- 订阅导入与更新（多策略重试）
- 定时自动更新订阅（可配置间隔）
- 节点组/节点切换
- 节点延迟测试（单节点/批量）
- 实时流量与连接信息
- 系统代理开关
- TUN 模式开关与状态检查
- 日志落盘与会话日志
- 退出清理（恢复系统代理与网络路由环境）
- 前后端弱耦合（UI 退出可保留后端 core）
- 退出三态策略（`always-on` / `always-off` / `query`）

## 当前运行模式

默认是 **Direct Core First**：

1. 优先接管可用的本地 Mihomo socket
2. 否则直接拉起 `verge-mihomo`
3. `clash-verge-service IPC` 为可选后备（设置 `VERGE_TUI_USE_SERVICE_IPC=1`）

## 快速开始

### 1. 编译

```bash
cargo build -p verge-tui --release
```

二进制位置：

```bash
./target/release/verge-tui
```

### 4. 生成安装目录并安装到系统

```bash
./scripts/build-install.sh
./install/install.sh
```

安装到当前用户目录：

```bash
./install/install.sh --user
```

### 2. 运行

```bash
./target/release/verge-tui
```

### 3. 常用命令（进入 `:` 命令模式）

- `help`
- `doctor`
- `import <url>`
- `reload proxies`
- `reload subscriptions`
- `autosub status`
- `autosub 60`
- `toggle sysproxy|tun`
- `sysproxy on|off|toggle`
- `delay selected`
- `delay all`
- `backend status`
- `cleanup`
- `save`

## 文档

- 使用文档：[`docs/USAGE.md`](docs/USAGE.md)
- 架构说明：[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- 开发文档：[`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md)
- 命令手册：[`docs/COMMANDS.md`](docs/COMMANDS.md)
- 发布流程：[`docs/RELEASE.md`](docs/RELEASE.md)
- 打包与 AUR：[`docs/PACKAGING.md`](docs/PACKAGING.md)

## AUR 一键流程

日常本地测试打包：

```bash
./scripts/aur-package.sh
```

发布到 AUR（自动更新校验和、刷新 `.SRCINFO`、提交并推送）：

```bash
./scripts/aur-push.sh
```

## Debian / APT

构建 `.deb`：

```bash
./scripts/build-deb.sh
```

构建静态 APT 仓库目录：

```bash
./scripts/build-apt-repo.sh
```

仓库已附带 GitHub Pages 发布工作流，可把 `dist/apt` 自动部署为可访问的 APT 源。

## 协议与来源

- 许可证：`GPL-3.0-only`，见 [`LICENSE`](LICENSE)
- 来源说明与衍生声明：见 [`NOTICE.md`](NOTICE.md)
