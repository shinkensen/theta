# Theta

Theta is an agentic voice assistant built with React, TypeScript, Rust, and Tauri. It implements a modular integrations architecture coupled with local speech to text, some rag based memory, an OpenRouter-powered (customizable though) agent, optional web search, and natural text-to-speech as well as a variety of integrations like one for Canvas, Spotify, Google Calendar, Notion, MineStrator, and Gmail.
## Architecture

Microphone -> cpal -> Vosk Lightweight Voice Recognition -> OpenRouter agent -> Tool Calls -> Edge Neural Text to Speech

## Prerequisites
- node.js and npm.
- rust and cargo
- Tauri, Webview2 and C++ Build Tools
- An OpenRouter API key
- Vosk Windows SDK and model.

## Installation

```powershell
git clone <repository-url>
cd theta
npm install
```

## Run and build

For testing purposes just use npm run tauri dev. MAKE SURE THE DEPENDENCIES ARE INSTALLED

also configure all the keys in the integrations dashboard, important ones are the Openrouter key and a firecrawl one

## Google Calendar setup
1. Create a google cloud project
2. Set up the google calendar extension to your project
3. Add a new desktop client
4. Take the generated client id and (optionally) client secret and paste it into the inputs for the connections page
## Canvas LMS setup

Theta has a canvas integration but its read only. It can be used to read your assignments, classes, and due dates.
Heres how to set it up:


1. Log into your instituitons canvas page
2. navigate to your user settings
3. Scroll down to where "Approved Integrations" is
4. Click "add new access token"
5. Take that access token and paste it into the place in the connections part of the app along with your institutions canvas url


## Memory and profile

Theta has two distinct local memory systems:

- **RAG memory** stores any documents that the user gives for future use.
- **About Me profile** automatically infers any hobbies, interests, or ideas that the user has and stores them here.

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
│   │   ├── canvas.rs          # Canvas authentication and read-only API
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

