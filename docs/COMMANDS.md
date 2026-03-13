# 命令手册

在 TUI 中按 `:` 进入命令模式。

## 诊断类

- `help`
  - 打开帮助面板
- `doctor`
  - 输出运行模式、endpoint、端口对齐状态、core 权限与 service socket 状态
- `logpath`
  - 输出主日志和当前会话日志路径
- `health`
  - 触发一次 mihomo 健康检查
- `adopt`
  - 尝试接管兼容链路（controller/socket）

## 订阅与配置

- `import <url>`
  - 导入订阅并自动尝试应用
- `reload proxies`
  - 刷新节点列表
- `reload subscriptions`
  - 刷新全部订阅
- `update [selected|all|<profile_uid>]`
  - 更新订阅（当前/全部/指定）
- `autosub [off|status|now|<minutes>]`
  - 定时订阅更新配置（关闭/查看状态/立即执行/设置分钟间隔）
- `use <profile_uid>`
  - 切换当前 profile

## 节点与测速

- `switch <group> <proxy>`
  - 在指定策略组内切换节点
- `delay <proxy|selected|all> [url] [timeout_ms]`
  - 测速单节点、当前选中节点或所有节点

示例：

```text
:delay selected
:delay all http://cp.cloudflare.com 5000
:switch 飞鸟云 美国01aws
```

## 运行模式与代理

- `mode <rule|global|direct>`
  - 修改 mihomo 运行模式
- `toggle sysproxy`
  - 切换系统代理
- `toggle tun`
  - 切换 TUN
- `sysproxy [on|off|toggle]`
  - 系统代理别名命令
- `cleanup`
  - 立即执行退出清理逻辑（关闭 TUN、关闭系统代理，尽可能恢复网络环境）
- `backend [status|start|stop|keep <on|off>]`
  - 后端控制（查看状态/拉起/停止/设置退出策略）
  - `backend stop` 会自动执行清理逻辑
  - `backend keep on/off` 是快捷方式，等价于 `always-on/always-off`
  - `backend policy <always-on|always-off|query>` 可设置三态策略

## 参数设置

- `set controller <url>`
- `set secret <secret>`
- `set mixed-port <port>`
- `set proxy-host <host>`
- `set auto-update <off|minutes>`
- `set cleanup-on-exit <on|off>`
- `set keep-core-on-exit <on|off>`
- `set backend-exit-policy <always-on|always-off|query>`

## 持久化与退出

- `save`
- `quit`

## 常见输入错误

- `sysproxy` 拼写正确，若输入 `toogle` 会报 `unknown command`
- `reload` 仅支持 `proxies` 或 `subscriptions`
