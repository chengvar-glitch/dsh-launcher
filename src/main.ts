import { listen } from "@tauri-apps/api/event";

interface LogLine {
  stream: "stdout" | "stderr";
  line: string;
}

const logsEl = document.getElementById("logs") as HTMLPreElement;
const statusEl = document.getElementById("status") as HTMLParagraphElement;
const titleEl = document.getElementById("title") as HTMLHeadingElement;
const spinnerEl = document.getElementById("spinner") as HTMLDivElement;

function appendLog(stream: string, line: string): void {
  const firstChild = logsEl.firstElementChild;
  if (firstChild?.classList.contains("muted")) firstChild.remove();

  const div = document.createElement("div");
  div.className = stream === "stderr" ? "stderr" : "stdout";
  div.textContent = line;

  // Detect the readiness line for nice styling (Rust also parses it).
  const isReady = /dsh web: http:\/\/127\.0\.0\.1:\d+/.test(line);
  if (isReady) div.classList.add("ready");

  logsEl.appendChild(div);
  logsEl.scrollTop = logsEl.scrollHeight;
}

function showError(message: string): void {
  spinnerEl.style.display = "none";
  titleEl.textContent = "启动失败";
  titleEl.classList.add("error-banner");
  statusEl.textContent = message;
}

async function main(): Promise<void> {
  await listen<LogLine>("log-line", (event) => {
    appendLog(event.payload.stream, event.payload.line);
  });

  await listen<string>("ready", (event) => {
    const url = event.payload;
    statusEl.textContent = "dsh web 已就绪，正在进入界面…";
    window.location.href = url;
  });

  await listen<string>("error", (event) => {
    showError(event.payload);
  });

  await listen<number | null>("child-exit", (event) => {
    const code = event.payload;
    spinnerEl.style.display = "none";
    titleEl.textContent = "dsh web 已退出";
    statusEl.textContent =
      code === null
        ? "进程被信号终止"
        : `退出码 ${code}，请查看上方日志。`;
    void (async () => {
      // Give Rust a moment to finish killing the process tree.
      await new Promise((resolve) => setTimeout(resolve, 2000));
      window.close();
    })();
  });
}

void main();
