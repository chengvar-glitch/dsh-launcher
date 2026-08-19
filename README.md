# DSH Launcher

Tauri 2 桌面壳：启动时拉起 `dsh web`（DeepSeek Harness 的 Web GUI），
等它就绪后把窗口指过去，启动日志实时展示，退出后常驻托盘。

MIT 许可，见 [LICENSE](LICENSE)。

## 原理

```
Tauri 窗口 → 本地 loading 页（显示启动日志）
   │
Rust: 先探测本地是否已有 dsh web 在跑（上次记住的 URL + lsof 端口扫描，
      发 HTTP GET 探 window.__DSH_BOOT__ 签名）
   │
   ├─ 命中 → 直接 attach 到该 URL，发 ready（不重复启动）
   │
   └─ 未命中 → spawn `dsh web --host 127.0.0.1 --port 0`（cwd = 指定目录）
         │  stdout/stderr 逐行 → 日志文件 + 事件推给 loading 页
         │  匹配到 "dsh web: http://127.0.0.1:<port>"（官方就绪信号）→ 发 ready
         │
loading 页收到 ready → window.location 跳转到该 URL（正式界面）
```

关闭窗口 = 隐藏到托盘（dsh 会话不中断）；托盘菜单"退出"才真正
结束进程树（按进程组 SIGTERM → SIGKILL）并退出应用。

健壮性：若 DeepSeek Harness 已在运行（本应用的托盘会话、终端里的
`dsh web`、或其他桌面壳），启动时会自动检测并直接进入已有界面，
不会重复拉起第二个实例。上次连接过的 URL 会记住，下次秒进。

## 运行

```sh
pnpm install          # 前端依赖
pnpm tauri dev        # 开发模式（Rust + Vite HMR）
pnpm tauri build      # 打包 .app / .dmg
```

## 配置（环境变量）

| 变量 | 默认值 | 说明 |
|---|---|---|
| `OPEN_DSH_CWD` | `/Users/chengvar/dev/deepseek-harness` | 运行 `dsh` 的工作目录（要有 node_modules） |
| `OPEN_DSH_CMD` | `pnpm dsh web --host 127.0.0.1 --port 0` | 启动命令，按空白分词，首词为可执行文件 |

例：

```sh
OPEN_DSH_CWD=/path/to/harness OPEN_DSH_CMD="pnpm dsh web" pnpm tauri dev
```

## 结构

```
src/                  loading 页（Vite + vanilla TS）
src-tauri/
  src/lib.rs          进程管理 / 日志流 / 就绪解析 / 托盘 / 退出清理
  tauri.conf.json     窗口、bundle、图标
  capabilities/       权限（loading 页仅需监听事件）
scripts/make-icon.mjs 零依赖生成图标源 PNG（可换品牌）
```

## 已知限制 / 待办

- 生产分发：`dsh` 是 node bin，正式安装包需捆绑 node runtime 或改用仓库的
  single-exe 构建产物（当前开发期直接用源码启动没问题）。
- 窗口标题 / 图标是占位品牌，按需替换（图标重新生成：改
  `scripts/make-icon.mjs` → `node scripts/make-icon.mjs` →
  `pnpm tauri icon src-tauri/icons/icon-source.png`）。
- dsh 中途崩溃会在 loading 页显示退出码并自动关窗；可在托盘菜单重启（待加）。
