# AGENTS.md

Tauri 2 桌面壳：启动时拉起 `dsh web`（DeepSeek Harness 的 Web GUI），
把 stdout/stderr 实时推给本地 loading 页，等到就绪信号后跳转正式界面；
关窗隐藏到托盘，托盘"退出"才结束 dsh 进程树并退出。

## 常用命令

```sh
pnpm install          # 前端依赖（pnpm workspace）
pnpm tauri dev        # 开发：Rust + Vite HMR（Vite 固定端口 1420，勿改）
pnpm build            # 前端构建 = tsc 类型检查 && vite build
pnpm tauri build      # 打包 .app / .dmg
cargo check           # 在 src-tauri/ 下做 Rust 检查（推荐改 Rust 后跑）
```

无 lint / 无测试框架。前端类型检查就是 `tsc`（strict，noUnusedLocals/Parameters）。

## 结构

```
src/main.ts            loading 页：监听 Rust 事件并跳转
src/styles.css         仅 loading 页样式
src-tauri/src/lib.rs   全部 Rust：进程管理 / 日志流 / 就绪解析 / 托盘 / 退出清理
src-tauri/tauri.conf.json      窗口与 bundle 配置
src-tauri/capabilities/default.json  权限：main 窗口仅 core:default + core:event:default
scripts/make-icon.mjs  零依赖生成图标源 PNG
```

## 关键机制（改这里先看 lib.rs）

- 就绪信号：stdout 出现 `dsh web: http://127.0.0.1:<port>`（`READY_PREFIX`）才发 `ready`，
  端口从行里解析，不可用固定端口。
- 进程树：spawn 用 `process_group(0)`；退出时 `kill(-pid, SIGTERM)` → 800ms 后 `SIGKILL`。
- 事件（loading 页只读）：`log-line` / `ready` / `error` / `child-exit`。
- 关窗 = 隐藏到托盘（`prevent_close`），`dsh` 会话不中断。
- 单实例插件：二次启动只把已有窗口带回来。

## 配置

| 变量 | 默认 | 说明 |
|---|---|---|
| `OPEN_DSH_CWD` | `/Users/chengvar/dev/deepseek-harness` | `dsh` 工作目录（要有 node_modules） |
| `OPEN_DSH_CMD` | `pnpm dsh web --host 127.0.0.1 --port 0` | 启动命令，空白分词，首词为可执行文件 |

## 约定

- 前端是 vanilla TS，无框架；注释 / UI 文案用中文。
- Rust 只依赖已列在 Cargo.toml 的 crate，新增依赖需确认打包体积（release 走 opt-level="s"）。
- 权限最小化：新 IPC 需同步更新 capabilities。
