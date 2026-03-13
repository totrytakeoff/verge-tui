# 开发文档

## 环境要求

- Rust toolchain（见 `rust-toolchain.toml`）
- Linux/macOS/Windows（TUN 与系统代理行为会有差异）

## 目录结构

- `apps/verge-tui`：主程序
- `crates/mihomo-client`：Mihomo API 客户端
- `crates/verge-core`：状态/订阅/系统代理
- `scripts/proxy-clean-linux.sh`：Linux 代理环境清理脚本

## 本地开发

```bash
cargo check -p verge-tui
cargo run -p verge-tui
```

发布构建：

```bash
cargo build -p verge-tui --release
```

生成安装目录：

```bash
./scripts/build-install.sh
```

本地 AUR 快速打包（当前工作区源码，跳过 checksum 校验）：

```bash
./scripts/aur-package.sh
```

AUR 发布模式（按 PKGBUILD source URL 更新 checksum 并生成 `.SRCINFO`）：

```bash
./scripts/aur-package.sh --release
```

## 调试建议

1. 首先执行 `:doctor`
2. 查看 `:logpath` 输出路径
3. 重点排查：
   - core endpoint 是否可达
   - mixed-port 是否一致
   - TUN 能力是否具备

## 代码约定

- 优先保持 UI 不阻塞
- 网络/IPC 失败快速返回并记录明确日志
- 功能优先，兼容链路（service IPC）作为可选后备
- 前后端默认弱耦合：UI 退出不必销毁 core（可通过 `keep-core-on-exit` 调整）
