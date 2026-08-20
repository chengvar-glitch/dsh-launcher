# DSH Launcher

Desktop wrapper for [DeepSeek Harness](https://github.com/deepseek-ai/DeepSeek-Harness) (`dsh web`).

English | [中文](README.zh-CN.md)

## Install

1. Install DeepSeek Harness:

   ```sh
   npm i -g @deepseek-ai/dsh
   ```

2. Download the latest installer from [Releases](https://github.com/chengvar-glitch/dsh-launcher/releases)
   (macOS `.dmg` / Windows `.msi` or `.exe`).

## Use

- Launch **DSH Launcher**. The first start runs `dsh web` automatically; subsequent starts reuse the running instance.
- Closing the window hides it to the menu bar; the `dsh` session keeps running.
- Select **Quit** from the tray/menu-bar icon to stop `dsh` and exit.

## Configure

By default the launcher runs:

```
dsh web --host 127.0.0.1 --port 0
```

Override with the `OPEN_DSH_CMD` environment variable (split on whitespace, first token is the executable, resolved from `PATH`):

```sh
OPEN_DSH_CMD="dsh web --host 127.0.0.1 --port 0"
```

## Build from source

```sh
pnpm install
pnpm tauri dev      # development
pnpm tauri build    # produces .app/.dmg, .msi/.exe, .deb/.rpm/AppImage
```

## License

[MIT](LICENSE)
