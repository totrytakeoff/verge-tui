# 发布流程（v0.x）

## 发布前检查

- `cargo check -p verge-tui`
- `cargo build -p verge-tui --release`
- 手动验证：
  - 导入订阅
  - 刷新节点与切换
  - 延迟测试（单节点/全部）
  - 系统代理 on/off
  - TUN on/off（具备权限时）

## 产物

- Linux/macOS/Windows 二进制（按目标平台交叉编译）

## 发布说明建议

至少包含：

- 新增功能
- 已知限制
- 升级/迁移提示
- TUN 权限说明
