# CLAUDE.md

Orientation for AI agents working on this repo. Keep it current when the architecture changes.

## What this is

A Tauri 2 desktop app that batch-renames images using a multimodal LLM. It is a GUI port of
the original `qwen-renamer.sh` (kept at the repo root for reference). The GUI is a single
screen: a config form plus a progress bar / status area.

## Stack & layout

- **Frontend:** vanilla TypeScript + Vite (no framework). Dark theme by default.
  - `index.html` — the form markup and run area.
  - `src/main.ts` — all UI logic: builds the config object, `invoke`s the backend, `listen`s
    for progress events, persists the form to `localStorage`, drives Start/Stop.
  - `src/styles.css` — dark palette via CSS variables on `:root`.
- **Backend:** Rust, in `src-tauri/`.
  - `src/lib.rs` — **the engine and all `#[tauri::command]`s.** This is where almost all
    backend work happens.
  - `src/main.rs` — thin entry point that calls `run()` from the lib.
  - `tauri.conf.json` — window, identifier, version, and bundle targets.
  - `capabilities/default.json` — permissions granted to the main window.

## How a run works (the contract)

1. `src/main.ts` gathers the form into a config object and calls
   `invoke("run_rename", { config })`.
2. `run_rename` (in `lib.rs`) scans the folder, then **sequentially** for each image:
   reads bytes → base64 → POSTs an OpenAI **Chat Completions** payload to `server_url` via
   `reqwest` → extracts `choices[0].message.content` → `sanitize()`s it → renames in place,
   or copies to `output_folder` when one is set.
3. After every image the backend emits a `renamer://progress` event. The frontend listens and
   updates the bar, counts, and status log. A final `status: "summary"` event ends the run.

**Event payload** (`ProgressEvent` in `lib.rs` ↔ `ProgressEvent` interface in `main.ts` — keep
them in sync): `{ done, total, file, status, message }` where `status` is one of
`info | renamed | copied | skipped | error | summary`.

**Cancellation:** `cancel_rename` flips an `AtomicBool` in `AppState`; the loop checks it
between images, so Stop takes effect after the current image finishes.

## Gotchas / decisions

- **Payload shape:** the original shell script used Responses-API field names
  (`input_text` / `input_image`). The Rust port uses the correct Chat Completions parts
  (`type: "text"` and `type: "image_url"` with a `data:` URL). Don't reintroduce the old shape.
- File I/O and HTTP run in Rust, which has full filesystem access — no `fs` capability is
  needed in `capabilities/default.json`. The frontend only needs `core`, `event`, and `dialog`.
- Settings persistence is plain `localStorage` (Tauri's webview storage survives restarts);
  no store plugin.
- Icons in `src-tauri/icons/` are **placeholder plain-magenta squares** — replace with real art.

## Adding a config field (common task)

1. Add the input to `index.html` and include it in `TEXT_FIELDS` (or extension handling) in
   `main.ts` so it persists, and add it to the config object in the submit handler.
2. Add the field to `RenameConfig` in `lib.rs` (serde field name must match the JS key).
3. Use it in `run_rename` / `query_llm`.

## Build & verify

- `npm install` then `npm run tauri dev` to run locally.
- `npm run build` type-checks the frontend; `cargo build --manifest-path src-tauri/Cargo.toml`
  compiles the backend.
- `npm run tauri build` produces installers for the current OS only. Cross-OS bundles are
  produced by `.github/workflows/release.yml` (matrix build → GitHub Release on push to main).
- macOS is ad-hoc self-signed for now (`APPLE_SIGNING_IDENTITY="-"`), not notarized.
