use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent,
};
use tauri_plugin_single_instance::init as single_instance;

/// Readiness line printed by the dsh web profile once the HTTP server is up.
/// The bundle's own source documents it as the supervisor readiness signal:
/// `dsh web: http://127.0.0.1:<port>` (plus an optional LAN suffix).
const READY_PREFIX: &str = "dsh web: http://127.0.0.1:";

struct AppState {
    /// Process id of the spawned `dsh` (leader of its process group).
    pid: Mutex<Option<i32>>,
    /// Kept so a watcher thread can reap the exit status.
    child: Mutex<Option<Child>>,
    /// Append target for the full launch transcript.
    log_file: Mutex<Option<File>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            pid: Mutex::new(None),
            child: Mutex::new(None),
            log_file: Mutex::new(None),
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

/// The command to boot the web GUI. Override via `OPEN_DSH_CMD`
/// (whitespace-split; first token is the program). The default boots the
/// harness's own `dsh web` profile with an OS-assigned port.
fn dsh_command() -> (String, Vec<String>) {
    match std::env::var("OPEN_DSH_CMD") {
        Ok(raw) => {
            let mut parts = raw.split_whitespace();
            let program = parts.next().unwrap_or("pnpm").to_string();
            let args: Vec<String> = parts.map(String::from).collect();
            (program, args)
        }
        Err(_) => (
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
                        let _ = app.emit("ready", url.clone());
                        append_log(&state, &format!("[shell] ready: {url}"));
                    }
                }
            }
        }
    });
}

fn spawn_dsh(app: &AppHandle, state: &Arc<AppState>) {
    let cwd = dsh_cwd();
    let (program, args) = dsh_command();

    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(&cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group so a single kill() call tears down the whole tree.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = format!("无法启动 {program}（cwd={}）：{error}", cwd.display());
            let _ = app.emit("error", message.clone());
            append_log(state, &format!("[shell] {message}"));
            return;
        }
    };

    let pid = child.id() as i32;
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
    let stdout = child.stdout.take();
    if let Some(out) = stdout {
        stream_lines(app.clone(), state.clone(), "stdout", Box::new(BufReader::new(out)), ready_sent.clone());
    }
    let stderr = child.stderr.take();
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
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

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
        .setup(|app| {
            let state = app.state::<Arc<AppState>>().inner().clone();

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
                    "[shell] open-dsh {} starting, log at {}",
                    env!("CARGO_PKG_VERSION"),
                    log_dir.join("launch.log").display()
                ),
            );

            spawn_dsh(app.handle(), &state);
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
