# Batch Image Renamer

A minimal desktop app that batch-renames images by asking a multimodal LLM to read each
one. Point it at a folder, describe what to extract in the prompt, and each image is renamed
(or copied) to `<prefix><model answer><.ext>`.

> _Business context / intended workflow: TODO — fill in._

The app talks to any OpenAI-compatible chat-completions endpoint with vision support
(e.g. [LM Studio](https://lmstudio.ai/), Ollama, llama.cpp server).

## Configuration

All settings live in the single form and are remembered between launches:

| Field | Notes |
| --- | --- |
| **Image folder** | Folder to scan (non-recursive). |
| **Output folder** | Optional. If set, renamed files are **copied** here and originals are left untouched. Empty = rename **in place**. |
| **Filename prefix** | Prepended to every new name. |
| **Server URL** | Chat-completions endpoint. Default `http://localhost:1234/v1/chat/completions`. The app auto-selects whichever model the server currently has loaded (queried from `/v1/models`). |
| **Prompt** | Instruction sent with each image. The model's reply becomes the filename. |
| **Max tokens** | Response length cap (`-1` = unlimited). |
| **Extensions** | Which image types to process (jpg/jpeg on by default). |

The model's reply is sanitized for use as a filename (anything outside `A–Z a–z 0–9 _ . -`
becomes `_`, repeats collapsed, ends trimmed). Files are processed one at a time. A file is
skipped only if the sanitized reply is empty or the file is already correctly named; on a name
clash with a *different* file the new name gets a numeric suffix (`name.jpg`, `name_1.jpg`,
`name_2.jpg`, …). Use **Stop** to halt after the current image.

## Development

Prerequisites: [Node.js](https://nodejs.org/) 18+, the [Rust toolchain](https://rustup.rs/),
and the [Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS
(on Debian/Ubuntu: `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev
libssl-dev libayatana-appindicator3-dev librsvg2-dev`).

```bash
npm install
npm run tauri dev      # run the app with hot-reload
```

## Building

```bash
npm run tauri build    # produces installers for the current OS
```

Bundles land in `src-tauri/target/release/bundle/`:

- **Linux:** `.AppImage` and `.deb`
- **macOS:** `.app` and `.dmg` (currently ad-hoc self-signed — see below)
- **Windows:** NSIS installer (built but untested)

Tauri does not cross-compile cleanly between desktop OSes — build each target on its own
platform, or use the GitHub Actions workflow (`.github/workflows/release.yml`), which builds
all three on a push to `main` and publishes a GitHub Release.

### macOS signing

Builds are **ad-hoc self-signed** for now (`APPLE_SIGNING_IDENTITY="-"`), not notarized.
On first launch users must right-click the app → **Open**, or clear the quarantine flag:

```bash
xattr -dr com.apple.quarantine "/Applications/Batch Image Renamer.app"
```
