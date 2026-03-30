# 使用文档

## 启动

```bash
./target/release/verge-tui
```

启动后按 `:` 进入命令模式。

启动时直接导入订阅：

```bash
./target/release/verge-tui --import "https://example.com/sub.yaml"
./target/release/verge-tui --import /path/to/subscriptions.txt
```

如果传入的是文件，程序会读取其中每一行的 `http/https` 链接并逐条导入。

## 键位

- `q`：退出（当退出策略为 `query` 时会弹出确认窗）
- `Tab / Shift+Tab / h / l`：切换主标签
- `j / k`：列表移动
- `Enter`：确认操作
- `Esc`：返回或关闭帮助

## 常用命令

### 诊断

- `help`：打开帮助面板
- `doctor`：输出运行模式、core 路径、端口与权限诊断
- `logpath`：输出日志文件路径
- `health`：手动健康检查

### 订阅与配置

- `import <url|file.txt>`：导入订阅，文件模式会按行读取 `http/https` 链接
- `reload subscriptions`：更新全部订阅
- `update selected`：更新当前订阅
- `autosub status`：查看自动更新状态
- `autosub 60`：每 60 分钟自动更新一次
- `autosub off`：关闭自动更新
- `use <profile_uid>`：切换 profile

### 节点与测速

- `reload proxies`：刷新节点列表
- `switch <group> <proxy>`：切换指定分组的节点
- `delay <proxy|selected|all> [url] [timeout_ms]`

### 代理模式

- `toggle sysproxy` / `sysproxy on|off|toggle`
- `toggle tun`
- `mode <rule|global|direct>`

### 持久化

- `save`
- `cleanup`：执行一次退出清理（关闭系统代理/TUN）
- `backend status`：查看后端状态
- `backend stop`：停止由 TUI 管理的后端 core（自动清理）
- `backend policy query`：退出时弹窗选择是否保留后端

## 环境变量

- `VERGE_TUI_HOME`：指定状态目录
- `VERGE_TUI_CORE_BIN`：指定 mihomo 内核路径
- `VERGE_TUI_USE_SERVICE_IPC=1`：启用 service IPC 后备路径
- `VERGE_TUI_INDEPENDENT=0`：关闭独立模式（通常不建议）

## TUN 权限（Linux）

推荐给 core 二进制授予能力：

```bash
sudo setcap cap_net_admin,cap_net_raw+ep /usr/bin/verge-mihomo
```

如果采用 service 模式，请确保对应 systemd 服务健康。

## 完整命令清单

见：[`docs/COMMANDS.md`](COMMANDS.md)
