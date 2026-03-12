# 软件架构

## 总览

`verge-tui` 由三层组成：

1. `apps/verge-tui`：TUI 应用层（交互、调度、状态展示）
2. `crates/mihomo-client`：Mihomo Controller API 客户端
3. `crates/verge-core`：本地状态、订阅导入、系统代理适配

## 控制链路

- UI 命令 -> `verge-tui` 调度 -> `mihomo-client` 调用 API
- 配置/订阅 -> `verge-core::StateStore` 落盘并维护

## 运行模式

### Direct Core First（默认）

1. 尝试接管已有本地 socket
2. 失败则直接拉起 `verge-mihomo`
3. 连通后加载 profile、刷新节点、启动流量订阅

### Service IPC（可选）

通过 `VERGE_TUI_USE_SERVICE_IPC=1` 启用：

- 先尝试通过 `clash-verge-service IPC` 启 core
- 若不可用，回退 direct core

## 关键数据

- 状态目录：`~/.config/verge-tui`
- 状态文件：`state.yaml`
- 订阅文件：`profiles/*.yaml`
- 运行时配置：`core-home/verge-tui-runtime.yaml`
- 日志：`logs/verge-tui.log` 与 `logs/session-*.log`

## 安全与权限

- 系统代理写入依赖 `sysproxy-rs`
- TUN 需要内核能力（Linux: `CAP_NET_ADMIN`, `CAP_NET_RAW`）
- 默认不要求 UI 进程以 root 运行
