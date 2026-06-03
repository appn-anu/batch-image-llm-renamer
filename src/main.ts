import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

/** Mirror of the Rust `ProgressEvent` payload emitted on `renamer://progress`. */
interface ProgressEvent {
  done: number;
  total: number;
  file: string;
  status: "info" | "renamed" | "copied" | "skipped" | "error" | "summary";
  message: string;
}

const STORAGE_KEY = "batch-image-llm-renamer.config";

// Form fields whose values we persist between launches.
const TEXT_FIELDS = [
  "image_folder",
  "output_folder",
  "filename_prefix",
  "server_url",
  "model_name",
  "prompt",
  "temperature",
  "max_tokens",
] as const;

// ---- Editable form defaults ----
// Change these to update what the form starts with on a fresh launch (or after
// saved settings are cleared). Saved settings take precedence once the user has
// edited a field. Extensions are intentionally NOT here — their defaults live in
// index.html, since changing them usually means changing which options exist too.
const FORM_DEFAULTS: Record<(typeof TEXT_FIELDS)[number], string> = {
  image_folder: "",
  output_folder: "",
  filename_prefix: "",
  server_url: "http://localhost:1234/v1/chat/completions",
  model_name: "qwen3-vl-8b-instruct",
  prompt:
    "Hi Qwen, please read the 'Plate' text from the label. The text should consist of a one digit number, one uppercase letter, and another one digit number. Print only this text. Thanks!",
  temperature: "0.7",
  max_tokens: "-1",
};

const $ = <T extends HTMLElement>(id: string): T =>
  document.getElementById(id) as T;

const form = $<HTMLFormElement>("config-form");
const fieldset = $<HTMLFieldSetElement>("config-fieldset");
const startBtn = $<HTMLButtonElement>("start-btn");
const stopBtn = $<HTMLButtonElement>("stop-btn");
const progressBar = $<HTMLProgressElement>("progress-bar");
const currentFile = $<HTMLSpanElement>("current-file");
const counts = $<HTMLSpanElement>("counts");
const statusLog = $<HTMLUListElement>("status-log");

let running = false;
const tally = { renamed: 0, copied: 0, skipped: 0, error: 0 };

// ---- Settings persistence (webview localStorage survives app restarts) ----

function saveConfig() {
  const data: Record<string, string | string[]> = {};
  for (const id of TEXT_FIELDS) {
    data[id] = $<HTMLInputElement>(id).value;
  }
  data.extensions = selectedExtensions();
  localStorage.setItem(STORAGE_KEY, JSON.stringify(data));
}

function applyDefaults() {
  for (const id of TEXT_FIELDS) {
    $<HTMLInputElement>(id).value = FORM_DEFAULTS[id];
  }
}

function loadConfig() {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return;
  try {
    const data = JSON.parse(raw);
    for (const id of TEXT_FIELDS) {
      if (typeof data[id] === "string") $<HTMLInputElement>(id).value = data[id];
    }
    if (Array.isArray(data.extensions)) {
      document.querySelectorAll<HTMLInputElement>("input.ext").forEach((cb) => {
        cb.checked = data.extensions.includes(cb.value);
      });
    }
  } catch {
    /* ignore malformed storage */
  }
}

function selectedExtensions(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLInputElement>("input.ext:checked"),
  ).map((cb) => cb.value);
}

// ---- Folder pickers ----

document.querySelectorAll<HTMLButtonElement>("button.browse").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const targetId = btn.dataset.target!;
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      $<HTMLInputElement>(targetId).value = selected;
      saveConfig();
    }
  });
});

$<HTMLButtonElement>("clear_output").addEventListener("click", () => {
  $<HTMLInputElement>("output_folder").value = "";
  saveConfig();
});

form.addEventListener("input", saveConfig);
form.addEventListener("change", saveConfig);

// ---- Status log + counts ----

function appendStatus(status: ProgressEvent["status"], text: string) {
  const li = document.createElement("li");
  li.className = status;
  li.textContent = text;
  statusLog.appendChild(li);
  statusLog.scrollTop = statusLog.scrollHeight;
}

function updateCounts() {
  counts.textContent = `${tally.renamed} renamed · ${tally.copied} copied · ${tally.skipped} skipped · ${tally.error} errors`;
}

function setRunning(state: boolean) {
  running = state;
  fieldset.disabled = state;
  startBtn.hidden = state;
  stopBtn.hidden = !state;
  stopBtn.disabled = false;
}

// ---- Progress events from the backend ----

listen<ProgressEvent>("renamer://progress", ({ payload }) => {
  if (payload.total > 0) {
    progressBar.max = payload.total;
    progressBar.value = payload.done;
  }

  switch (payload.status) {
    case "renamed":
      tally.renamed++;
      break;
    case "copied":
      tally.copied++;
      break;
    case "skipped":
      tally.skipped++;
      appendStatus("skipped", `SKIP  ${payload.file} — ${payload.message}`);
      break;
    case "error":
      tally.error++;
      appendStatus("error", `ERROR ${payload.file} — ${payload.message}`);
      break;
    case "summary":
      appendStatus("summary", payload.message);
      setRunning(false);
      currentFile.textContent = "Done";
      break;
    case "info":
      break;
  }

  if (payload.status !== "summary" && payload.file) {
    currentFile.textContent = `Processing ${payload.file} (${payload.done}/${payload.total})`;
  }
  updateCounts();
});

// ---- Start / Stop ----

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  if (running) return;

  const config = {
    image_folder: $<HTMLInputElement>("image_folder").value.trim(),
    output_folder: $<HTMLInputElement>("output_folder").value.trim() || null,
    filename_prefix: $<HTMLInputElement>("filename_prefix").value,
    server_url: $<HTMLInputElement>("server_url").value.trim(),
    model_name: $<HTMLInputElement>("model_name").value.trim(),
    prompt: $<HTMLTextAreaElement>("prompt").value,
    temperature: parseFloat($<HTMLInputElement>("temperature").value) || 0,
    max_tokens: parseInt($<HTMLInputElement>("max_tokens").value, 10) || -1,
    extensions: selectedExtensions(),
  };

  if (!config.image_folder) {
    appendStatus("error", "Please choose an image folder.");
    return;
  }
  if (config.extensions.length === 0) {
    appendStatus("error", "Select at least one file extension.");
    return;
  }

  // Reset run state.
  statusLog.innerHTML = "";
  tally.renamed = tally.copied = tally.skipped = tally.error = 0;
  progressBar.value = 0;
  progressBar.max = 1;
  updateCounts();
  currentFile.textContent = "Starting…";
  setRunning(true);

  try {
    await invoke("run_rename", { config });
  } catch (err) {
    appendStatus("error", `Run failed: ${err}`);
    setRunning(false);
    currentFile.textContent = "Failed";
  }
});

stopBtn.addEventListener("click", async () => {
  stopBtn.disabled = true;
  currentFile.textContent = "Stopping after current image…";
  await invoke("cancel_rename");
});

applyDefaults();
loadConfig();
