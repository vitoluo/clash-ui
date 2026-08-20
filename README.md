# Clash UI

<p align="center">
  <a href="https://slint.dev/"><img src="https://slint.dev/logo/MadeWithSlint-logo-light.svg" alt="Made with Slint"></a>
</p>

基于 Rust 和 Slint 构建的 Mihomo/Clash 桌面客户端，提供配置、代理、规则、连接、日志、系统代理和 TUN 等功能。

## Slint 署名

本项目使用 [Slint](https://github.com/slint-ui/slint) 构建桌面用户界面，并依据 [Slint Royalty-free Desktop, Mobile, and Web Applications License](https://slint.dev/agreements/slint-royalty-free-license.pdf) 在本公开页面展示官方 `#MadeWithSlint` 署名徽章。徽章使用遵循 [Slint Brand Guidelines](https://slint.dev/brand-guidelines)。

## 构建与运行

需要安装 Rust stable 工具链。在项目根目录执行：

```bash
cargo run
```

构建发布版本：

```bash
cargo build --release
```

## 覆写数组 key 规则

覆写或配置文件中的数组字段默认由后者整体替换前者。如需在已有数组上增删元素，可使用以下兄弟键指令；指令键仅在合并时生效，不会写入最终的 `config.yaml`：

- `key`：整体替换 `key` 数组（默认行为）。
- `key::^`：将数组元素前置到 `key` 数组开头。
- `key::$`：将数组元素追加到 `key` 数组末尾。
- `key::N`（`N` 为非负整数）：将数组元素插入到 `key` 数组第 `N` 个索引之后；当 `N` 大于或等于当前长度时追加到末尾；`key` 不存在或不是数组时按空数组处理。

上述指令支持任意嵌套层级。同一合并层中先处理普通键，再处理指令键，确保替换先于插入生效。

## 主要依赖与致谢

本项目使用或参考以下开源项目：

| 项目 | 用途 | 来源 |
| --- | --- | --- |
| [Slint](https://github.com/slint-ui/slint) | 声明式桌面用户界面框架 | [slint.dev](https://slint.dev/) |
| [slintcn](https://github.com/zero-sq/slintcn) | Slint UI 组件注册表及组件来源 | [文档](https://zero-sq.github.io/slintcn/) |
| [lucide-slint](https://github.com/cnlancehu/lucide-slint) | Slint 图标库 | [crates.io](https://crates.io/crates/lucide-slint) |
| [mihomo](https://github.com/MetaCubeX/mihomo) | Clash 兼容的代理核心 | [官方文档](https://wiki.metacubex.one/) |
| [zashboard](https://github.com/Zephyruso/zashboard) | Mihomo/Clash Web UI | [GitHub](https://github.com/Zephyruso/zashboard) |
| [clash-verge-rev/sysproxy-rs](https://github.com/clash-verge-rev/sysproxy-rs) | 系统代理设置 | [GitHub](https://github.com/clash-verge-rev/sysproxy-rs/) |

感谢上述项目及其贡献者。

## 许可证

本项目自身代码以 [MIT License](LICENSE) 发布。

第三方依赖、核心程序、图标及其他外部资源不受本项目 MIT License 自动覆盖，具体许可条款以各自上游项目为准。
