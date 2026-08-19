import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";

interface LogLine {
  stream: "stdout" | "stderr";
  line: string;
}

const statusEl = document.getElementById("status") as HTMLParagraphElement;
const actionsEl = document.getElementById("actions") as HTMLDivElement;
const btnLog = document.getElementById("btn-log") as HTMLButtonElement;
const btnRetry = document.getElementById("btn-retry") as HTMLButtonElement;

let booted = false;
let navigating = false;

function setStatus(text: string): void {
  statusEl.textContent = text;
}

function showError(message: string): void {
  document.body.classList.add("is-error");
  statusEl.textContent = message;
  statusEl.classList.add("is-error");
  actionsEl.hidden = false;
}

async function main(): Promise<void> {
  // Log lines are not rendered — they only drive the boot stage label so the
  // user sees real progress without a noisy log panel.
  await listen<LogLine>("log-line", () => {
    if (!booted) {
      booted = true;
      setStatus("booting…");
    }
  });

  await listen<string>("ready", (event) => {
    if (navigating) return;
    navigating = true;
    document.body.classList.add("is-ready");
    setStatus("ready");
    // Let the fill animation play out before leaving the loading page.
    setTimeout(() => {
      window.location.href = event.payload;
    }, 550);
  });

  await listen<string>("error", (event) => {
    showError(event.payload);
  });

  await listen<number | null>("child-exit", () => {
    showError("failed to start");
    void (async () => {
      await new Promise((resolve) => setTimeout(resolve, 2000));
      window.close();
    })();
  });

  btnLog.addEventListener("click", () => {
    void invoke("open_log");
  });

  btnRetry.addEventListener("click", () => {
    void invoke("retry_boot").then(() => window.location.reload());
  });

  // Tell Rust the page is listening. If it found an already-running
  // harness, it delivers the ready URL now (attach path, race-free).
  await emit("page-ready");
}

void main();
