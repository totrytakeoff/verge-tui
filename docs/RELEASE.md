# 发布流程（v0.x）

## 发布前检查

- `cargo check -p verge-tui`
- `cargo build -p verge-tui --release`
- `./scripts/build-install.sh`
- 手动验证：
  - 导入订阅
  - 刷新节点与切换
  - 延迟测试（单节点/全部）
  - 系统代理 on/off
  - TUN on/off（具备权限时）

## 建议发布步骤

1. 更新版本号（`Cargo.toml` / 变更日志）。
2. 推送代码并创建 GitHub tag（例如 `v0.1.1`）。
3. 执行 AUR 一键发布：

```bash
./scripts/aur-push.sh
```

4. 如需 Debian/APT 分发，构建 `.deb` 与 APT 仓库目录：

```bash
./scripts/build-deb.sh
./scripts/build-apt-repo.sh
```

5. 如需自动托管 APT 仓库，启用 GitHub Pages，并确保 `apt-pages.yml` 可运行。

该脚本会自动执行：

- `./scripts/aur-package.sh --release`
- 更新 `PKGBUILD` 中 `sha256sums`
- 生成 `.SRCINFO`
- 推送到 `ssh://aur@aur.archlinux.org/<pkgname>.git`

## 仅更新 AUR 元数据（不推送）

```bash
./scripts/aur-package.sh --release
```

## 仅推送（跳过重新打包）

```bash
./scripts/aur-push.sh --no-package
```

## 产物

- Linux/macOS/Windows 二进制（按目标平台交叉编译）
- Debian `.deb`
- 静态 APT 仓库目录（`dist/apt`）

## 发布说明建议

至少包含：

- 新增功能
- 已知限制
- 升级/迁移提示
- TUN 权限说明
