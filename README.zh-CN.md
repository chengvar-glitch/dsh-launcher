# DSH Launcher

[DeepSeek Harness](https://github.com/deepseek-ai/DeepSeek-Harness)（`dsh web`）的桌面壳。

[English](README.md) | 中文

## 安装

1. 安装 DeepSeek Harness：

   ```sh
   npm i -g @deepseek-ai/dsh
   ```

2. 从 [Releases](https://github.com/chengvar-glitch/dsh-launcher/releases)
   下载最新安装包（macOS 为 `.dmg`，Windows 为 `.msi` 或 `.exe`）。

## 使用

- 启动 **DSH Launcher**：首次会自动运行 `dsh web`，之后会直接复用已在运行的实例。
- 关闭窗口只是隐藏到菜单栏，`dsh` 会话继续运行。
- 在托盘/菜单栏图标里选择 **退出** 才会结束 `dsh` 并退出应用。

## 配置

默认启动命令：

```
dsh web --host 127.0.0.1 --port 0
```

可通过环境变量 `OPEN_DSH_CMD` 覆盖（按空白分词，首词为可执行文件，从 `PATH` 查找）：

```sh
OPEN_DSH_CMD="dsh web --host 127.0.0.1 --port 0"
```

## 从源码构建

```sh
pnpm install
pnpm tauri dev      # 开发模式
pnpm tauri build    # 产出 .app/.dmg、.msi/.exe、.deb/.rpm/AppImage
```

## 许可

[MIT](LICENSE)
