# Theta

Theta is a Windows-focused desktop voice assistant built with React, TypeScript, Rust, and Tauri 2. It combines local speech recognition and local memory with an OpenRouter-powered conversational agent, optional web search, Google Calendar tools, system inspection, and natural text-to-speech.

> **Project status:** Theta is under active development. The current native speech setup is Windows-specific, and several integrations use third-party or unofficial services whose behaviour may change.

## Highlights

- **Local speech-to-text:** microphone capture through `cpal` and offline recognition with Vosk.
- **Conversational agent:** OpenRouter chat completions with parallel tool calling and fallback routing.
- **Natural speech:** Microsoft Edge Read Aloud neural voices, preferring UK English, with system speech synthesis fallback.
- **Local memory:** embedding-free retrieval using BM25, character trigrams, rank fusion, and recency.
- **Adaptive profile:** an editable “About me” profile learns interests, hobbies, projects, preferences, and recurring topics.
- **Google Calendar:** list calendars and events, create/update/delete events, and natural-language quick add.
- **System tools:** inspect processes, system statistics, and listening ports; protected tools can stop processes or run commands.
- **Desktop integration:** global hotkey, tray behaviour, optional autostart, and close-to-tray.
- **Safety controls:** write/destructive tools request confirmation by default; auto-approve is explicit and disabled by default.
- **Diagnostics:** an in-app debug console captures frontend, agent, speech, and backend events.

## Architecture

```text
Microphone
   │
   ▼
Rust: cpal audio capture ──► Vosk offline STT
   │                             │
   │ Tauri events                ▼
   └──────────────────────► React conversation UI
                                  │
              local RAG/profile ─┤
                                  ▼
                         OpenRouter agent loop
                                  │
                    tool calls ───┼──► Tauri/Rust commands
                                  │
                                  ▼
                    Edge neural TTS ──► system voice fallback
```

### Frontend (`src/`)

The React frontend owns the application shell, conversation state, OpenRouter agent loop, tool orchestration, profile extraction, Firecrawl-backed web search, Edge speech synthesis, settings UI, confirmation prompts, and debug console.

### Rust backend (`src-tauri/`)

The Tauri backend owns microphone capture and Vosk recognition, system/process inspection, local RAG and profile persistence, settings, Google OAuth and Calendar API calls, global shortcuts, tray behaviour, and autostart.

## Local and remote processing

| Capability | Processing location | Data that may leave the device |
|---|---|---|
| Microphone capture and Vosk transcription | Local | None |
| RAG indexing/search and About Me profile | Local | Selected context may be included in an OpenRouter prompt |
| Agent reasoning | OpenRouter | User message, conversation context, selected memory/profile context, and tool results |
| Web search | Firecrawl | Search query |
| Google Calendar | Google APIs | OAuth data and requested Calendar content |
| Neural speech | Unofficial Edge Read Aloud endpoint | Reply text and selected voice |
| System voice fallback | WebView/operating system | Depends on the installed voice provider |
| System/process inspection | Local | Results may be returned to the agent as tool output |

“Local-first” does **not** mean fully offline. Recognition, retrieval, profile storage, and system operations are local; the conversational model and enabled integrations require network access.

## Safety and approval model

Read-only tools can execute without prompting. The following protected actions require an in-app confirmation by default:

- Running shell commands.
- Terminating processes.
- Creating, updating, deleting, or quick-adding Calendar events.
- Ingesting files/text into local memory or deleting memory.

The **Auto-approve protected actions** setting bypasses those prompts. It defaults to **off**. Enable it only if you trust the selected model and understand that commands, process termination, Calendar writes, and memory changes can then run without individual review. This approval layer is not an operating-system sandbox.

## Prerequisites

The current native setup targets Windows 11 and requires:

- [Node.js](https://nodejs.org/) and npm.
- The [Rust toolchain](https://rustup.rs/) with Cargo.
- [Tauri 2 prerequisites for Windows](https://v2.tauri.app/start/prerequisites/), including Microsoft C++ Build Tools and WebView2.
- A working microphone.
- An OpenRouter API key for conversational responses.
- Network access for OpenRouter and any enabled remote integrations.
- The Vosk Windows SDK and English recognition model described below.

The Tauri configuration can produce multiple bundle targets, but the vendored Vosk linking logic in `src-tauri/build.rs` is currently Windows-specific. Other operating systems require native Vosk setup changes.

## Installation

```powershell
git clone <repository-url>
cd theta
npm install
```

### 1. Install the Vosk Windows SDK

Download `vosk-win64-0.3.45.zip` from the [Vosk API releases](https://github.com/alphacep/vosk-api/releases) and extract it so this file exists:

```text
src-tauri/vosk/vosk-win64-0.3.45/libvosk.lib
```

The directory should also contain these runtime libraries:

```text
libvosk.dll
libgcc_s_seh-1.dll
libstdc++-6.dll
libwinpthread-1.dll
```

`src-tauri/build.rs` adds this SDK to the Windows linker search path and copies the DLLs beside debug/release executables. If Theta is running during a rebuild, Windows may lock those DLLs and Cargo will print copy warnings; close Theta before rebuilding if fresh DLLs must be copied.

### 2. Install the Vosk model

Download `vosk-model-small-en-us-0.15` from the [Vosk model catalogue](https://alphacephei.com/vosk/models) and extract it to:

```text
src-tauri/vosk-models/vosk-model-small-en-us-0.15/
├── am/
├── conf/
├── graph/
└── ivector/
```

Theta searches several development-relative locations but the path above is the canonical repository layout. Startup fails with a descriptive message when the model cannot be found.

### 3. Configure frontend integrations

Create `.env` in the repository root. Use your own values—never commit this file.

> **Security warning:** every `VITE_*` value is compiled into the frontend bundle and can be recovered by someone who can inspect the application. These values are **not secrets**. Use narrowly scoped development credentials, avoid privileged or long-lived keys, and rotate any exposed value.

```dotenv
VITE_OPENROUTER_API_KEY=<your-openrouter-api-key>
VITE_FIRECRAWL_KEY=<optional-firecrawl-api-key>
VITE_EDGE_TRUSTED_TOKEN=<optional-edge-read-aloud-token>
```

| Variable | Required | Purpose |
|---|---:|---|
| `VITE_OPENROUTER_API_KEY` | Yes | Agent responses and automatic profile extraction |
| `VITE_FIRECRAWL_KEY` | Only for web search | Firecrawl search requests |
| `VITE_EDGE_TRUSTED_TOKEN` | No | Overrides the Edge Read Aloud token used by the speech client |

The Edge integration has an application fallback token, but Edge Read Aloud is unofficial and may stop working when Microsoft changes its protocol. Do not assume that fallback is suitable for production distribution.

## Run and build

### Desktop development

```powershell
npm run tauri dev
```

This starts Vite on port `1420`, compiles the Rust backend, and opens the desktop application. Use this mode to exercise microphone, Calendar, persistence, system tools, tray, and other native Tauri commands.

### Frontend-only development

```powershell
npm run dev
```

This starts only Vite. The UI loads in a browser, but features that invoke Tauri commands will not work outside the desktop runtime.

### Validation and packaging

```powershell
# Type-check and create the frontend production bundle
npm run build

# Run Rust tests
cargo test --manifest-path src-tauri/Cargo.toml

# Build platform installers/bundles
npm run tauri build

# Preview the frontend bundle only
npm run preview
```

Frontend output is written to `dist/`. Tauri build artifacts are produced under `src-tauri/target/release/` and its `bundle/` subdirectory.

## First-run configuration

Open **Settings** in Theta and review:

| Setting | Default | Notes |
|---|---|---|
| Model | `openrouter/free` | Any compatible OpenRouter model ID can be entered |
| Voice | `en-GB-SoniaNeural` | UK neural voice; selectable alternatives are provided |
| Speak replies | On | Disable for text-only responses |
| Local memory context | On | Adds relevant local RAG results to agent context |
| Web search | On | Requires `VITE_FIRECRAWL_KEY` when invoked |
| Auto-approve | Off | Bypasses prompts for protected tools—use carefully |
| Global hotkey | `CommandOrControl+Shift+Space` | Registered by the Tauri global-shortcut plugin |
| Listen when summoned | On | Starts recognition when the hotkey reveals Theta |
| Close to tray | On | Closing hides the main window instead of quitting |
| Launch at login | Off | Uses the Tauri autostart plugin |
| Default calendar | `primary` | Google Calendar ID used when none is specified |

Use **Test this voice** to test TTS and inspect the Debug view if Edge falls back to the system voice.

## Google Calendar setup

Theta implements the OAuth 2.0 installed-application flow with PKCE and state validation.

1. In [Google Cloud Console](https://console.cloud.google.com/), create or select a project.
2. Enable the **Google Calendar API**.
3. Configure the OAuth consent screen. If the application is in Testing mode, add your Google account as a test user.
4. Create an OAuth client with application type **Desktop app**. Do not use a Web application client.
5. In Theta, open **Settings → Google Calendar**, paste the client ID and optional client secret, and save.
6. Select **Connect** and complete sign-in in the browser.

Theta binds an ephemeral loopback listener on `127.0.0.1` and supplies that generated address as the redirect URI. A Web application client generally causes `redirect_uri_mismatch` because it expects a fixed registered redirect. The requested Calendar scope permits event reads and writes.

Google client configuration, refresh/access tokens, expiry, and account email are persisted in `google_auth.json` under Theta's platform app-data directory. This is local JSON storage, not demonstrated operating-system credential-vault encryption. Use **Disconnect** to remove the saved Google authentication state.

## Speech

### Recognition

Vosk recognition is offline after the model is installed. Rust captures the system's default input device through `cpal`, converts samples for the recognizer, and emits partial/final transcript events to the frontend.

### Synthesis

Theta connects directly to the unofficial Edge Read Aloud WebSocket, builds validated SSML, synthesizes bounded text segments with two-segment prefetch, and plays ordered MP3 blobs. It prefers `en-GB-SoniaNeural`. If connection, synthesis, or playback fails, Theta reports the technical reason to Debug and uses the best available UK system/browser voice.

## Agent tools

| Category | Tools | Confirmation required by default |
|---|---|---:|
| Web | Search the web | No |
| Local memory | Search/list memory | No |
| Local memory writes | Ingest text/file, forget memory | Yes |
| Calendar reads | List calendars/events | No |
| Calendar writes | Create, update, delete, quick add | Yes |
| System inspection | Statistics, process list, listening ports | No |
| System mutation | Kill process, run command | Yes |

The agent executes independent read-only calls concurrently. Confirmation-sensitive calls are serialized while manual approval is enabled because the UI displays one pending confirmation. With auto-approve enabled, protected calls can run without that queue. Failed protected operations remain visible for diagnosis; successful or denied protected activity cards are removed from chat.

The loop permits up to eight model/tool rounds per user turn. Retryable OpenRouter provider failures can fall back to `openrouter/free` when a different configured model fails.

## Memory and profile

Theta has two distinct local memory systems:

- **RAG memory** stores user-ingested text or files and retrieves relevant chunks using lexical and fuzzy ranking. Users can list and delete stored documents.
- **About Me profile** automatically extracts non-sensitive interests, hobbies, projects, preferences, and recurring topics. Repeated evidence raises confidence; facts can be removed individually or cleared together.

Profile learning excludes sensitive categories such as credentials, exact addresses, health, religion, politics, sexuality, and finances. A profile extractor still sends the current conversation turn and current profile to the selected OpenRouter model.

## Local data

Theta uses Tauri's per-user platform app-data directory. On Windows this is resolved by Tauri for the application identifier `com.govind.theta`; do not rely on a hard-coded user path.

| File | Contents |
|---|---|
| `settings.json` | Hotkey, desktop behaviour, model, voice, tool toggles, and Calendar defaults |
| `profile.json` | Learnt About Me facts and evidence metadata |
| `rag.json` | Indexed local-memory chunks and metadata |
| `google_auth.json` | Google OAuth client configuration and tokens |

Conversation transcript state is maintained in the running frontend and is not written by the current hook to a transcript file. Closing/restarting Theta clears that conversation context.

To reset specific data, use the corresponding UI actions: clear About Me, forget RAG documents, or disconnect Google Calendar. For a complete reset, quit Theta and remove its app-data directory. Back up anything needed first; deleting that directory is irreversible.

## Repository structure

```text
.
├── src/
│   ├── agent/                 # OpenRouter loop, tools, profile extraction
│   ├── hooks/                 # Voice-assistant orchestration
│   ├── voice/                 # Edge TTS and system fallback
│   ├── App.tsx                # Application shell and views
│   └── App.css                # Calm-premium visual system
├── src-tauri/
│   ├── src/
│   │   ├── calendar.rs        # Google OAuth and Calendar API
│   │   ├── procs.rs           # Process/system/port tools
│   │   ├── profile.rs         # Structured About Me persistence
│   │   ├── rag.rs             # Local retrieval and storage
│   │   ├── settings.rs        # Persisted preferences
│   │   ├── stt.rs             # cpal/Vosk recognition
│   │   └── lib.rs             # Tauri setup, tray, hotkey, commands
│   ├── vosk/                  # Windows native SDK (local setup)
│   ├── vosk-models/           # Recognition model (local setup)
│   ├── build.rs               # Vosk linker and DLL-copy setup
│   └── tauri.conf.json        # Window, bundle, and build configuration
├── package.json
└── README.md
```

## Troubleshooting

### Vosk model not found

Ensure the extracted directory is exactly:

```text
src-tauri/vosk-models/vosk-model-small-en-us-0.15
```

It must directly contain `am`, `conf`, `graph`, and `ivector`; avoid an extra nested directory created by extraction.

### `libvosk.lib` not found

Check that `src-tauri/vosk/vosk-win64-0.3.45/libvosk.lib` exists. The build script intentionally stops with a path-specific message when the SDK is absent.

### DLL copy warnings (`os error 32`)

A running `theta.exe` has likely locked Vosk runtime DLLs. Tests may still pass using existing copies. Quit Theta before rebuilding when those DLLs need replacement.

### Port 1420 is already in use

Another Vite/Theta development process is running. Close that process before starting a second `npm run tauri dev`; do not terminate unrelated processes.

### No microphone input or STT errors

Confirm Windows microphone privacy access, select a valid default input device, and check Theta's Debug view for the selected device/configuration and Vosk events.

### OpenRouter is not configured or returns no message

Set `VITE_OPENROUTER_API_KEY`, restart the Vite/Tauri development process (environment variables are read at build time), and check that the selected model is available. Theta migrates one legacy provider model to `openrouter/free` and logs provider failures/fallback attempts in Debug.

### Web search is unavailable

Set `VITE_FIRECRAWL_KEY` and restart/rebuild. Alternatively, disable **Allow web search tool** in Settings.

### Google `redirect_uri_mismatch`

Use a Google OAuth client of type **Desktop app**. Theta intentionally uses a fresh ephemeral loopback redirect; it is incompatible with most Web application client configurations.

### Google Calendar HTTP 411 or malformed event arguments

Current code sends an explicit zero content length for bodyless Quick Add POSTs. Structured event tools normalize JSON-string events and Google-style `{ "dateTime": "…" }`/`{ "date": "…" }` nodes. Rebuild/restart Theta if an older running backend still reports these errors.

### Edge speech falls back to a robotic/system voice

Open Debug and inspect the latest speech diagnostic. Common causes include network filtering, an expired protocol/token, invalid voice configuration, or Microsoft changing the unofficial endpoint. The fallback is expected resilience behaviour, not a local neural model.

### OAuth or settings changes appear stale

The running Tauri backend must be restarted after Rust changes. Frontend environment variables also require restarting Vite because they are compiled into the client.

## Security considerations

- `VITE_OPENROUTER_API_KEY`, `VITE_FIRECRAWL_KEY`, and any other `VITE_*` values are embedded in distributable frontend assets. Do not treat them as confidential.
- OpenRouter calls currently originate from frontend JavaScript with `dangerouslyAllowBrowser: true`; production deployments should proxy privileged credentials through a controlled backend.
- OAuth tokens are stored in a local JSON file rather than a demonstrated OS credential vault.
- The Google integration requests broad Calendar access for read/write tools.
- Auto-approve can authorize shell execution, process termination, Calendar writes, and memory modifications without per-action review.
- Ingesting a file sends its path to the Rust backend and may later expose retrieved excerpts to the model as context.
- The Tauri content security policy is currently disabled (`"csp": null`). Review and restrict it before distributing the application broadly.
- Edge Read Aloud is an unofficial integration. Verify its licensing, terms, token handling, and redistribution suitability before release.
- Third-party responses, search pages, Calendar content, memory text, and tool output are untrusted data. They must not be treated as application instructions outside the agent's controlled tool policy.

If any real credential has been committed, logged, bundled, or shared, rotate it. Removing it from a current `.env` file does not remove it from prior commits or already-built artifacts.

## Current limitations

- Native Vosk linking and runtime assets are set up specifically for Windows.
- The bundled frontend architecture exposes `VITE_*` credentials by design.
- Edge neural speech depends on an unofficial protocol.
- Speech finals are submitted as produced by Vosk; short fragmented utterances can become separate requests.
- Conversation history is session-local rather than durable.
- The UI and agent implementation are still relatively monolithic and under active development.
- Packaging must account for the Vosk model and native libraries; verify release bundles on a clean machine.

## Contributing and validation

Before submitting changes, run:

```powershell
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

For speech, Calendar, tray, global-shortcut, or process-tool changes, also perform a desktop smoke test with `npm run tauri dev`. Never commit `.env`, Google auth data, app-data JSON, generated transcripts, API keys, or user-specific memory.

## License and third-party components

This repository currently has no root license file. Do not assume permission to redistribute Theta until a project license is added. Vosk, its model files, Tauri, React, and all other dependencies retain their respective licenses; review their terms and provide required notices before publishing binaries.
