import { emit, listen } from "@tauri-apps/api/event";

interface LogLine {
  stream: "stdout" | "stderr";
  line: string;
}

const statusEl = document.getElementById("status") as HTMLParagraphElement;

let booted = false;

function setStatus(text: string): void {
  statusEl.textContent = text;
}

function showError(message: string): void {
  document.body.classList.add("is-error");
  statusEl.textContent = message;
  statusEl.classList.add("is-error");
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
    setStatus("ready");
    window.location.href = event.payload;
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

  // Tell Rust the page is listening. If it found an already-running
  // harness, it delivers the ready URL now (attach path, race-free).
  await emit("page-ready");
}

void main();
