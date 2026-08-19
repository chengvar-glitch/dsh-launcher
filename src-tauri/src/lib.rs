use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Listener, Manager, RunEvent,
};
use tauri_plugin_single_instance::init as single_instance;

/// Readiness line printed by the dsh web profile once the HTTP server is up.
/// The bundle's own source documents it as the supervisor readiness signal:
/// `dsh web: http://127.0.0.1:<port>` (plus an optional LAN suffix).
const READY_PREFIX: &str = "dsh web: http://127.0.0.1:";

/// HTML signature only a live `dsh web` serves: its boot payload. Used to
/// tell an already-running harness instance apart from any other localhost
/// HTTP server when probing for it.
const BOOT_SIGNATURE: &str = "window.__DSH_BOOT__";

/// Probe timeouts: keep startup snappy even when many ports are listening.
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const PROBE_READ_TIMEOUT: Duration = Duration::from_millis(400);
/// Upper bound on how many localhost listeners get probed at startup.
const MAX_SCAN_PORTS: usize = 64;

struct AppState {
    /// Process id of the spawned `dsh` (leader of its process group).
    pid: Mutex<Option<i32>>,
    /// Kept so a watcher thread can reap the exit status.
    child: Mutex<Option<Child>>,
    /// Append target for the full launch transcript.
    log_file: Mutex<Option<File>>,
    /// URL of an already-running harness, held until the loading page
    /// signals it is listening (avoids racing the webview's event listeners).
    pending_ready: Mutex<Option<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            pid: Mutex::new(None),
            child: Mutex::new(None),
            log_file: Mutex::new(None),
            pending_ready: Mutex::new(None),
        }
    }
}

#[derive(Clone, Serialize)]
struct LogLine {
    stream: String,
    line: String,
}

/// The working directory the shell runs `dsh` in. Override via `OPEN_DSH_CWD`.
fn dsh_cwd() -> PathBuf {
    std::env::var("OPEN_DSH_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/Users/chengvar/dev/deepseek-harness"))
}

/// macOS GUI apps launched from Finder do not inherit the shell PATH, so the
/// `pnpm`/`node` shebangs cannot be resolved. Collect the usual Node install
/// locations (nvm, Homebrew, /usr/local) and prepend them to PATH so spawning
/// `dsh` works no matter how this app was launched.
fn augment_path() -> String {
    let mut dirs: Vec<String> = Vec::new();

    if let Ok(home) = std::env::var("HOME") {
        let nvm = PathBuf::from(&home).join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(&nvm) {
            let mut versions: Vec<_> = entries.filter_map(Result::ok).collect();
            versions.sort_by_key(|e| e.file_name());
            if let Some(latest) = versions.pop() {
                dirs.push(latest.path().join("bin").display().to_string());
            }
        }
    }
    dirs.push("/opt/homebrew/bin".to_string());
    dirs.push("/usr/local/bin".to_string());

    let mut path = std::env::var("PATH").unwrap_or_default();
    if !dirs.is_empty() {
        let extra = dirs.join(":");
        path = if path.is_empty() {
            extra
        } else {
            format!("{extra}:{path}")
        };
    }
    path
}

/// How the shell decided to boot the harness.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LaunchMode {
    /// `OPEN_DSH_CMD` was set explicitly.
    CustomCommand,
    /// `OPEN_DSH_CWD` was set explicitly: run the workspace's `dsh` there.
    CustomCwd,
    /// Default source checkout exists → `pnpm dsh web` in it.
    SourceDir,
    /// A global `dsh` binary was found on PATH (npm -g @deepseek-ai/dsh).
    PathDsh,
}

/// Whether `name` is an executable reachable on `path` (colon-separated).
fn find_on_path(path: &str, name: &str) -> bool {
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .any(|dir| Path::new(dir).join(name).is_file())
}

/// Decide how to boot the harness:
/// 1. explicit `OPEN_DSH_CWD` / `OPEN_DSH_CMD` wins;
/// 2. otherwise, if the default source checkout exists, run `pnpm dsh web` there;
/// 3. otherwise, use a globally installed `dsh` from PATH (`npm i -g @deepseek-ai/dsh`);
/// 4. nothing found → still attempt the source checkout so the failure
///    message can point the user at both install options.
fn resolve_launch_mode() -> LaunchMode {
    if std::env::var("OPEN_DSH_CMD").is_ok() {
        return LaunchMode::CustomCommand;
    }
    if std::env::var("OPEN_DSH_CWD").is_ok() {
        return LaunchMode::CustomCwd;
    }
    let cwd = dsh_cwd();
    let source_ok = cwd.join("package.json").is_file() && cwd.join("node_modules").is_dir();
    if source_ok {
        return LaunchMode::SourceDir;
    }
    if find_on_path(&augment_path(), "dsh") {
        return LaunchMode::PathDsh;
    }
    LaunchMode::SourceDir
}

/// The command to boot the web GUI. Override via `OPEN_DSH_CMD`
/// (whitespace-split; first token is the program). The default boots the
/// harness's own `dsh web` profile with an OS-assigned port.
fn dsh_command(mode: LaunchMode) -> (String, Vec<String>) {
    match mode {
        LaunchMode::CustomCommand => {
            let raw = std::env::var("OPEN_DSH_CMD").unwrap_or_default();
            let mut parts = raw.split_whitespace();
            let program = parts.next().unwrap_or("pnpm").to_string();
            let args: Vec<String> = parts.map(String::from).collect();
            (program, args)
        }
        LaunchMode::PathDsh => (
            "dsh".to_string(),
            vec![
                "web".into(),
                "--host".into(),
                "127.0.0.1".into(),
                "--port".into(),
                "0".into(),
            ],
        ),
        _ => (
            "pnpm".to_string(),
            vec![
                "dsh".into(),
                "web".into(),
                "--host".into(),
                "127.0.0.1".into(),
                "--port".into(),
                "0".into(),
            ],
        ),
    }
}

/// Extract the actual listening port from a readiness line, if present.
fn ready_port(line: &str) -> Option<String> {
    let rest = line.get(line.find(READY_PREFIX)? + READY_PREFIX.len()..)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

fn append_log(state: &AppState, line: &str) {
    let mut guard = state.log_file.lock().unwrap();
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Path of the file remembering the last harness URL, so the next launch can
/// attach in one probe instead of rescanning every listener.
fn last_url_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("last-harness-url.txt")
}

fn remember_url(app: &AppHandle, url: &str) {
    let path = last_url_path(app);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, url);
}

fn remembered_url(app: &AppHandle) -> Option<String> {
    fs::read_to_string(last_url_path(app))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Minimal HTTP probe: does `host:port` serve the dsh web boot page?
/// Identified by `__DSH_BOOT__`, which only dsh web injects.
fn probe_url(host: &str, port: u16) -> bool {
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, PROBE_CONNECT_TIMEOUT) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(PROBE_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PROBE_READ_TIMEOUT));
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 8192];
    let mut total = 0usize;
    loop {
        match stream.read(&mut buf[total..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                total += n;
                if total >= buf.len() {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&buf[..total]).contains(BOOT_SIGNATURE)
}

/// Enumerate TCP listeners on the loopback interface (macOS/BSD `lsof`).
/// Returns the numeric ports; wildcard (`*`) listeners are kept because the
/// harness can bind them and still answer on 127.0.0.1.
fn loopback_listener_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    let Ok(output) = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
    else {
        return ports;
    };
    if !output.status.success() {
        return ports;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        // Format: `... TCP 127.0.0.1:3080 (LISTEN)` or `... TCP *:3080 (LISTEN)`
        if let Some(idx) = line.find("TCP ") {
            let rest = &line[idx + 4..];
            let addr = rest.split_whitespace().next().unwrap_or("");
            if let Some((host, port)) = addr.rsplit_once(':') {
                if host == "127.0.0.1" || host == "*" {
                    if let Ok(port) = port.parse::<u16>() {
                        if !ports.contains(&port) {
                            ports.push(port);
                        }
                    }
                }
            }
        }
    }
    ports
}

/// Find a DeepSeek Harness web instance that is already running on localhost.
/// Fast path: the URL remembered by a previous run, probed for liveness.
/// Slow path: scan every loopback listener for the harness boot signature.
/// Probes run concurrently so one unresponsive listener cannot stall startup.
fn detect_existing_harness(app: &AppHandle) -> Option<String> {
    if let Some(url) = remembered_url(app) {
        if let Some(port) = url.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
            if probe_url("127.0.0.1", port) {
                return Some(url);
            }
        }
    }

    let ports = loopback_listener_ports();
    if ports.is_empty() {
        return None;
    }

    let found = Mutex::new(None);
    std::thread::scope(|scope| {
        for port in ports.into_iter().take(MAX_SCAN_PORTS) {
            let found = &found;
            scope.spawn(move || {
                if probe_url("127.0.0.1", port) {
                    let mut guard = found.lock().unwrap();
                    if guard.is_none() {
                        *guard = Some(format!("http://127.0.0.1:{port}"));
                    }
                }
            });
        }
    });

    let url = found.into_inner().unwrap();
    if let Some(url) = &url {
        remember_url(app, url);
    }
    url
}

/// Read a piped stream line by line: persist to the log file, forward to the
/// loading page, and emit the `ready` event on the first readiness line.
fn stream_lines(
    app: AppHandle,
    state: Arc<AppState>,
    stream: &'static str,
    reader: Box<dyn BufRead + Send>,
    ready_sent: Arc<Mutex<bool>>,
) {
    std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            append_log(&state, &format!("[{stream}] {line}"));
            let _ = app.emit("log-line", LogLine { stream: stream.into(), line: line.clone() });
            if stream == "stdout" {
                if let Some(port) = ready_port(&line) {
                    let mut sent = ready_sent.lock().unwrap();
                    if !*sent {
                        *sent = true;
                        let url = format!("http://127.0.0.1:{port}");
                        remember_url(&app, &url);
                        let _ = app.emit("ready", url.clone());
                        append_log(&state, &format!("[shell] ready: {url}"));
                    }
                }
            }
        }
    });
}

fn spawn_dsh(app: &AppHandle, state: &Arc<AppState>) {
    let mode = resolve_launch_mode();
    let cwd = dsh_cwd();
    let (program, args) = dsh_command(mode);

    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(&cwd)
        .env("PATH", augment_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group so a single kill() call tears down the whole tree.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mode_label = match mode {
        LaunchMode::CustomCommand => "OPEN_DSH_CMD",
        LaunchMode::CustomCwd => "OPEN_DSH_CWD",
        LaunchMode::SourceDir => "source checkout",
        LaunchMode::PathDsh => "global dsh (PATH)",
    };
    append_log(state, &format!("[shell] launch mode: {mode_label}"));

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let hint = match mode {
                LaunchMode::PathDsh => "（全局 dsh 未找到）",
                _ => "（源码目录不可用；可 npm i -g @deepseek-ai/dsh，或用 OPEN_DSH_CWD/OPEN_DSH_CMD 指定）",
            };
            let message = format!("无法启动 {program}（cwd={}）{hint}：{error}", cwd.display());
            let _ = app.emit("error", message.clone());
            append_log(state, &format!("[shell] {message}"));
            return;
        }
    };

    let pid = child.id() as i32;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    append_log(
        state,
        &format!(
            "[shell] spawned `{program} {}` in {} (pid {pid})",
            args.join(" "),
            cwd.display()
        ),
    );
    *state.pid.lock().unwrap() = Some(pid);
    *state.child.lock().unwrap() = Some(child);

    let ready_sent = Arc::new(Mutex::new(false));
    if let Some(out) = stdout {
        stream_lines(app.clone(), state.clone(), "stdout", Box::new(BufReader::new(out)), ready_sent.clone());
    }
    if let Some(err) = stderr {
        stream_lines(app.clone(), state.clone(), "stderr", Box::new(BufReader::new(err)), ready_sent);
    }

    // Watcher: reap the exit status once dsh terminates and tell the page.
    let watcher_app = app.clone();
    let watcher_state = state.clone();
    std::thread::spawn(move || {
        let child = watcher_state.child.lock().unwrap().take();
        let Some(mut child) = child else { return };
        let code = child.wait().ok().and_then(|status| status.code());
        *watcher_state.pid.lock().unwrap() = None;
        let _ = watcher_app.emit("child-exit", code);
        append_log(&watcher_state, &format!("[shell] dsh exited (code {code:?})"));
    });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "重启 dsh", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &restart, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("bundle icon must be present");

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "restart" => restart_dsh(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}

/// Kill the current dsh tree, respawn it, and point the window back at the
/// loading page so the new boot sequence is visible. Used by the tray
/// "restart" item and the loading page's retry button.
fn restart_dsh(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    append_log(&state, "[shell] restart requested");
    kill_dsh_tree(&state);
    std::thread::sleep(Duration::from_millis(300));
    spawn_dsh(app, &state);
    if let Some(window) = app.get_webview_window("main") {
        let loading = if cfg!(debug_assertions) {
            "http://localhost:1420/"
        } else {
            "tauri://localhost/"
        };
        if let Ok(url) = loading.parse() {
            let _ = window.navigate(url);
        }
    }
}

/// Open the launch log in the system's default viewer (used by the loading
/// page's "查看日志" button after a failed boot).
#[tauri::command]
fn open_log(app: AppHandle) {
    let log_dir = app.path().app_log_dir().unwrap_or_else(|_| std::env::temp_dir());
    let path = log_dir.join("launch.log");
    if path.exists() {
        let _ = Command::new("open").arg(&path).spawn();
    }
}

/// Reboot the harness after a failed boot (used by the retry button).
#[tauri::command]
fn retry_boot(app: AppHandle) {
    restart_dsh(&app);
}

/// Disable the rubber-band ("jelly") overscroll on the macOS WKWebView.
///
/// macOS WKWebView has no public `scrollView` (that is iOS-only), so the
/// bounce cannot be turned off natively. Instead we inject a user script
/// (WKUserScript) that sets `overscroll-behavior: none` on every page the
/// webview loads — including the external `dsh web` page, which a Tauri
/// initialization script would never reach.
///
/// The WKWebView must already exist, so this runs deferred after the event
/// loop starts (calling it from `setup` crashes: the webview is not attached
/// yet and messaging it raises an ObjC exception that cannot unwind through
/// the extern "C" boundary).
#[cfg(target_os = "macos")]
fn disable_overscroll_bounce(window: &tauri::WebviewWindow) {
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let _ = window.with_webview(move |webview| {
            use objc2::rc::Retained;
            use objc2::runtime::AnyObject;
            use objc2::MainThreadMarker;
            use objc2_web_kit::{
                WKUserContentController, WKUserScript, WKUserScriptInjectionTime,
                WKWebView, WKWebViewConfiguration,
            };

            // `with_webview` runs the closure on the main thread, so the
            // marker can always be obtained here.
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };

            let raw: *mut AnyObject = webview.inner().cast();
            let webview = unsafe { Retained::retain(raw.cast::<WKWebView>()) };
            let Some(webview) = webview else { return };
            let config: Retained<WKWebViewConfiguration> =
                unsafe { webview.configuration() };
            let controller: Retained<WKUserContentController> =
                unsafe { config.userContentController() };
            let source = objc2_foundation::NSString::from_str(
                "var s=document.createElement('style');s.textContent='html,body{overscroll-behavior:none!important}';document.head.appendChild(s);",
            );
            // `initWithSource:...` is `method_family = init`: it is declared
            // as an associated function taking `Allocated<Self>` first.
            let script: Retained<WKUserScript> = unsafe {
                WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    mtm.alloc::<WKUserScript>(),
                    &source,
                    WKUserScriptInjectionTime::AtDocumentEnd,
                    false,
                )
            };
            unsafe { controller.addUserScript(&script) };
        });
    });
}

/// Kill the whole dsh process group; escalate to SIGKILL after a short grace.
fn kill_dsh_tree(state: &AppState) {
    let pid = match *state.pid.lock().unwrap() {
        Some(pid) => pid,
        None => return,
    };
    // Negative pid targets the process group (spawned with process_group(0)).
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(800));
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(single_instance(|app, _args, _cwd| {
            // A second launch just brings the existing window back.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(Arc::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![open_log, retry_boot])
        .setup(|app| {
            let state = app.state::<Arc<AppState>>().inner().clone();

            // Turn off the rubber-band overscroll so the wrapped web page
            // does not bounce (this applies to the external dsh web page too).
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                disable_overscroll_bounce(&window);
            }

            let log_dir = app.path().app_log_dir()?;
            fs::create_dir_all(&log_dir)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_dir.join("launch.log"))?;
            *state.log_file.lock().unwrap() = Some(file);
            append_log(
                &state,
                &format!(
                    "[shell] DSH Launcher {} starting, log at {}",
                    env!("CARGO_PKG_VERSION"),
                    log_dir.join("launch.log").display()
                ),
            );

            // The loading page emits `page-ready` once its event listeners
            // are registered; only then is it safe to deliver a ready URL
            // found by the attach probe (avoids racing the webview load).
            let ready_app = app.handle().clone();
            let ready_state = state.clone();
            app.handle().listen("page-ready", move |_| {
                let url = ready_state.pending_ready.lock().unwrap().take();
                if let Some(url) = url {
                    let _ = ready_app.emit("ready", url);
                }
            });

            // Robustness: if a DeepSeek Harness web instance is already
            // running (this app's tray session, a terminal `dsh web`, or
            // another desktop shell), attach to it directly instead of
            // starting a duplicate.
            if let Some(url) = detect_existing_harness(app.handle()) {
                append_log(&state, &format!("[shell] existing dsh web found: {url}; attaching"));
                let _ = app.emit(
                    "log-line",
                    LogLine {
                        stream: "stdout".into(),
                        line: format!("检测到已运行的 DeepSeek Harness（{url}），直接进入界面…"),
                    },
                );
                *state.pending_ready.lock().unwrap() = Some(url.clone());
                // Watchdog: if the page-ready handshake is somehow missed
                // (e.g. an early listen failure), still deliver the URL.
                let watchdog_app = app.handle().clone();
                let watchdog_state = state.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1500));
                    let url = watchdog_state.pending_ready.lock().unwrap().take();
                    if let Some(url) = url {
                        let _ = watchdog_app.emit("ready", url);
                    }
                });
            } else {
                spawn_dsh(app.handle(), &state);
            }
            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // The window is a shell: closing it hides to the tray instead of
            // killing the dsh session. Quit comes from the tray menu.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                let state = app.state::<Arc<AppState>>().inner().clone();
                kill_dsh_tree(&state);
            }
        });
}
