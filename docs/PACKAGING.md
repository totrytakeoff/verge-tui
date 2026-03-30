# 打包与仓库维护

本文档说明 `verge-tui` 的本地打包、AUR 元数据生成，以及 Debian/APT 发布流程。

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

## Debian / APT

APT 不存在类似 AUR 的统一中央仓库提交流程。常见发布方式有两种：

- 自托管 APT 仓库：把 `.deb` 和索引文件发布到 GitHub Pages、S3、自己的站点等
- Launchpad PPA：适合 Ubuntu 系，但它是 PPA 体系，不是通用 Debian 仓库

当前仓库已经提供自托管 APT 所需脚本。

### `build-deb.sh`

用途：从当前源码生成 `.deb` 包。

```bash
./scripts/build-deb.sh
```

常用参数：

- `--out <dir>`：输出目录，默认 `./dist/deb`
- `--arch <arch>`：覆盖 Debian 架构，例如 `amd64`、`arm64`
- `--pkgrel <rel>`：Debian 包修订号，默认 `1`
- `--no-core`：不把 `verge-mihomo` 一起打进包

输出示例：

- `dist/deb/verge-tui_0.1.1-1_amd64.deb`

### `build-apt-repo.sh`

用途：把 `dist/deb/*.deb` 组织成一个可静态托管的 APT 仓库目录。

```bash
./scripts/build-apt-repo.sh
```

常用参数：

- `--deb-dir <dir>`：输入 `.deb` 目录，默认 `./dist/deb`
- `--repo-dir <dir>`：输出仓库目录，默认 `./dist/apt`
- `--origin <name>`：Release 的 `Origin`
- `--label <name>`：Release 的 `Label`
- `--suite <name>`：默认 `stable`
- `--codename <name>`：默认 `stable`
- `--component <name>`：默认 `main`
- `--gpg-key <id>`：用指定 GPG key 生成 `Release.gpg` 和 `InRelease`
- `--public-key-name <name>`：导出的仓库公钥文件名前缀

如果环境变量 `GPG_PASSPHRASE` 已设置，脚本会用 loopback 模式签名。

输出目录示例：

- `dist/apt/pool/main/v/verge-tui/*.deb`
- `dist/apt/dists/stable/main/binary-amd64/Packages`
- `dist/apt/dists/stable/Release`
- `dist/apt/dists/stable/InRelease`
- `dist/apt/dists/stable/Release.gpg`
- `dist/apt/verge-tui-archive-keyring.asc`
- `dist/apt/verge-tui-archive-keyring.gpg`

### GitHub Release 集成

当前 `release.yml` 已在 Linux tag 构建中自动：

- 生成 `.deb`
- 生成 `dist/apt` 仓库目录 artifact
- 把 `.deb` 上传到 GitHub Release

### GitHub Pages 托管 APT 仓库

仓库已提供：

- [apt-pages.yml](/home/myself/workspace/ClashT/verge-tui/.github/workflows/apt-pages.yml)

作用：

- 在 `v*` tag push 时自动构建 `.deb`
- 自动生成 `dist/apt`
- 自动部署到 GitHub Pages 的 `/apt/` 路径
- 如果配置了签名 secret，会自动签名 `Release/InRelease`

启用前提：

1. 在 GitHub 仓库设置里启用 `Pages`
2. Source 选择 `GitHub Actions`
3. 如需签名，配置以下 GitHub Secrets：

- `APT_GPG_PRIVATE_KEY`
- `APT_GPG_KEY_ID`
- `APT_GPG_PASSPHRASE`

启用后，APT 仓库地址通常会是：

```text
https://<your-user>.github.io/<repo>/apt
```

### 建议发布路线

1. 本地验证：`cargo check -p verge-tui`
2. 打 tag：例如 `v0.1.1`
3. GitHub Actions 自动生成二进制和 `.deb`
4. 选择一种仓库发布方式：

- 简单分发：直接让 Debian/Ubuntu 用户从 GitHub Release 下载 `.deb`
- APT 仓库：把 `dist/apt` 发布到 GitHub Pages 或其他静态托管

### 如果要给用户配置 APT 源

假设你把 `dist/apt` 发布到了：

```text
https://totrytakeoff.github.io/verge-tui/apt
```

未签名时可以这样配置：

```bash
echo "deb [trusted=yes] https://totrytakeoff.github.io/verge-tui/apt stable main" | sudo tee /etc/apt/sources.list.d/verge-tui.list
sudo apt update
sudo apt install verge-tui
```

启用 GPG 签名后，推荐这样配置：

```bash
curl -fsSL https://totrytakeoff.github.io/verge-tui/apt/verge-tui-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/verge-tui-archive-keyring.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/verge-tui-archive-keyring.gpg] https://totrytakeoff.github.io/verge-tui/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/verge-tui.list
sudo apt update
sudo apt install verge-tui
```
