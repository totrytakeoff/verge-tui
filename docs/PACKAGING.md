# 打包与 AUR 维护

本文档说明 `verge-tui` 的本地打包、AUR 元数据生成与推送流程。

## 相关脚本

- `scripts/aur-package.sh`
- `scripts/aur-push.sh`

## `aur-package.sh`

用途：构建 AUR 包并生成/更新元数据。

### 常用命令

本地快速打包（使用当前工作区代码，跳过 checksum）：

```bash
./scripts/aur-package.sh
```

发布模式（按 `PKGBUILD` 的 `source` 重新计算 checksum，并生成 `.SRCINFO`）：

```bash
./scripts/aur-package.sh --release
```

带依赖检查：

```bash
./scripts/aur-package.sh --release --deps
```

### 参数

- `--release`：发布模式。
- `--deps`：不传 `--nodeps` 给 `makepkg`。

## `aur-push.sh`

用途：一键发布 AUR（默认先执行 `aur-package.sh --release`）。

### 常用命令

一键打包并推送：

```bash
./scripts/aur-push.sh
```

仅推送（不重新打包）：

```bash
./scripts/aur-push.sh --no-package
```

演练模式（不 push）：

```bash
./scripts/aur-push.sh --dry-run
```

自定义提交信息：

```bash
./scripts/aur-push.sh --message "verge-tui 0.1.1-1"
```

### 参数

- `--no-package`：跳过打包步骤。
- `--deps`：透传给 `aur-package.sh`。
- `--message <msg>`：AUR 提交信息。
- `--aur-dir <path>`：AUR 元数据目录，默认 `./aur/verge-tui`。
- `--repo-url <url>`：AUR 仓库地址，默认 `ssh://aur@aur.archlinux.org/<pkgname>.git`。
- `--branch <name>`：推送分支，默认 `master`。
- `--dry-run`：不 push，仅本地提交验证。

## 推荐日常流程

1. 日常开发阶段：`./scripts/aur-package.sh`
2. 准备发布时：先打 GitHub tag（与 `PKGBUILD` 的 `pkgver/source` 对齐）
3. 发布 AUR：`./scripts/aur-push.sh`

## 注意事项

- AUR 不会自动从 GitHub 同步，必须手动提交 AUR 仓库。
- `--release` 模式依赖 `PKGBUILD` 的 `source` 可访问且版本存在。
- 首次推送前需配置好 AUR 账号 SSH key。
