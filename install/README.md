# install 目录说明

该目录用于生成可分发安装包内容。

- `install.sh`: 安装到系统（默认 `/usr/local`，支持 `--user`）
- `uninstall.sh`: 从系统卸载
- `bin/`: 打包后的可执行文件（由 `scripts/build-install.sh` 生成）
- `docs/`: 打包文档（由 `scripts/build-install.sh` 生成）

生成打包内容：

```bash
./scripts/build-install.sh
```

安装：

```bash
./install/install.sh
```
