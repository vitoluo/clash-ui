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

## 主要依赖与致谢

本项目使用或参考以下开源项目：

| 项目 | 用途 | 来源 |
| --- | --- | --- |
| [Slint](https://github.com/slint-ui/slint) | 声明式桌面用户界面框架 | [slint.dev](https://slint.dev/) |
| [slintcn](https://github.com/zero-sq/slintcn) | Slint UI 组件注册表及组件来源 | [文档](https://zero-sq.github.io/slintcn/) |
| [lucide-slint](https://github.com/cnlancehu/lucide-slint) | Slint 图标库 | [crates.io](https://crates.io/crates/lucide-slint) |
| [mihomo](https://github.com/MetaCubeX/mihomo) | Clash 兼容的代理核心 | [官方文档](https://wiki.metacubex.one/) |
| [clash-verge-rev/sysproxy-rs](https://github.com/clash-verge-rev/sysproxy-rs) | 系统代理设置 | [GitHub](https://github.com/clash-verge-rev/sysproxy-rs/) |

感谢上述项目及其贡献者。

## 许可证

本项目自身代码以 [MIT License](LICENSE) 发布。

第三方依赖、核心程序、图标及其他外部资源不受本项目 MIT License 自动覆盖，具体许可条款以各自上游项目为准。
