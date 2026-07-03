import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Recording from "./components/Recording";
import "./App.css";


type View = "home" | "system" | "files" | "notes" | "voice";

interface SystemInfo {
  os: string;
  arch: string;
  uptime_secs: number;
  hostname: string;
}

function App() {
  const [currentView, setCurrentView] = useState<View>("home");
  const [greetName, setGreetName] = useState("");
  const [greetMsg, setGreetMsg] = useState("");
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [sysError, setSysError] = useState("");
  const [dirPath, setDirPath] = useState(".");
  const [dirEntries, setDirEntries] = useState<string[]>([]);
  const [dirError, setDirError] = useState("");
  const [noteContent, setNoteContent] = useState("");
  const [notePath, setNotePath] = useState("");
  const [noteMsg, setNoteMsg] = useState("");

  async function handleGreet() {
    if (!greetName.trim()) return;
    const result = await invoke<string>("greet", { name: greetName });
    setGreetMsg(result);
  }

  async function handleGetSystemInfo() {
    setSysError("");
    try {
      const info = await invoke<SystemInfo>("get_system_info");
      setSystemInfo(info);
    } catch (e) {
      setSysError(String(e));
    }
  }

  async function handleListDir() {
    setDirError("");
    try {
      const entries = await invoke<string[]>("list_directory", { path: dirPath });
      setDirEntries(entries);
    } catch (e) {
      setDirError(String(e));
      setDirEntries([]);
    }
  }

  async function handleSaveNote() {
    setNoteMsg("");
    if (!notePath.trim() || !noteContent.trim()) {
      setNoteMsg("Please provide both a file path and content.");
      return;
    }
    try {
      const result = await invoke<string>("write_file_content", {
        path: notePath,
        content: noteContent,
      });
      setNoteMsg(result);
    } catch (e) {
      setNoteMsg(`Error: ${String(e)}`);
    }
  }

  async function handleReadNote() {
    setNoteMsg("");
    if (!notePath.trim()) {
      setNoteMsg("Please provide a file path to read.");
      return;
    }
    try {
      const result = await invoke<string>("read_file_content", { path: notePath });
      setNoteContent(result);
      setNoteMsg("File loaded successfully!");
    } catch (e) {
      setNoteMsg(`Error: ${String(e)}`);
    }
  }

  return (
    <div className="app-layout">
      {/* Sidebar */}
      <nav className="sidebar">
        <div className="sidebar-header">
          <h1>θ Theta</h1>
        </div>
        <ul className="nav-list">
          <li>
            <button
              className={`nav-btn ${currentView === "home" ? "active" : ""}`}
              onClick={() => setCurrentView("home")}
            >
              🏠 Home
            </button>
          </li>
          <li>
            <button
              className={`nav-btn ${currentView === "system" ? "active" : ""}`}
              onClick={() => setCurrentView("system")}
            >
              💻 System
            </button>
          </li>
          <li>
            <button
              className={`nav-btn ${currentView === "files" ? "active" : ""}`}
              onClick={() => setCurrentView("files")}
            >
              📂 Files
            </button>
          </li>
          <li>
            <button
              className={`nav-btn ${currentView === "notes" ? "active" : ""}`}
              onClick={() => setCurrentView("notes")}
            >
              📝 Notes
            </button>
          </li>
          <li>
            <button
              className={`nav-btn ${currentView === "voice" ? "active" : ""}`}
              onClick={() => setCurrentView("voice")}
            >
              🎙️ Voice
            </button>
          </li>
        </ul>
        <div className="sidebar-footer">
          <span>Tauri + React + TS</span>
        </div>
      </nav>

      {/* Main Content */}
      <main className="main-content">
        {currentView === "home" && (
          <div className="view">
            <h2>Welcome to Theta</h2>
            <p className="subtitle">
              A desktop app built with Tauri, React, and TypeScript.
            </p>

            <div className="card">
              <h3>Say Hello</h3>
              <p>Test the Rust backend by sending a greeting:</p>
              <form
                className="inline-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  handleGreet();
                }}
              >
                <input
                  type="text"
                  value={greetName}
                  onChange={(e) => setGreetName(e.target.value)}
                  placeholder="Enter your name..."
                />
                <button type="submit">Greet</button>
              </form>
              {greetMsg && <p className="result-msg">{greetMsg}</p>}
            </div>

            <div className="card">
              <h3>Quick Start</h3>
              <ul className="feature-list">
                <li>
                  <strong>🎙️ Voice</strong> — Record, transcribe, chat, and speak back
                </li>
                <li>
                  <strong>💻 System</strong> — View system information from Rust
                </li>
                <li>
                  <strong>📂 Files</strong> — Browse directories on your machine
                </li>
                <li>
                  <strong>📝 Notes</strong> — Read and write files via Tauri commands
                </li>
              </ul>
            </div>
          </div>
        )}

        {currentView === "system" && (
          <div className="view">
            <h2>System Information</h2>
            <p>Fetch system details from the Rust backend.</p>

            <button onClick={handleGetSystemInfo} className="action-btn">
              Get System Info
            </button>

            {sysError && <p className="error-msg">{sysError}</p>}

            {systemInfo && (
              <div className="card info-grid">
                <div className="info-item">
                  <span className="info-label">OS</span>
                  <span className="info-value">{systemInfo.os}</span>
                </div>
                <div className="info-item">
                  <span className="info-label">Architecture</span>
                  <span className="info-value">{systemInfo.arch}</span>
                </div>
                <div className="info-item">
                  <span className="info-label">Hostname</span>
                  <span className="info-value">{systemInfo.hostname}</span>
                </div>
                <div className="info-item">
                  <span className="info-label">Unix Timestamp</span>
                  <span className="info-value">{systemInfo.uptime_secs}</span>
                </div>
              </div>
            )}
          </div>
        )}

        {currentView === "files" && (
          <div className="view">
            <h2>File Browser</h2>
            <p>List the contents of a directory on your machine.</p>

            <form
              className="inline-form"
              onSubmit={(e) => {
                e.preventDefault();
                handleListDir();
              }}
            >
              <input
                type="text"
                value={dirPath}
                onChange={(e) => setDirPath(e.target.value)}
                placeholder="Enter directory path (e.g. . or C:\Users)..."
              />
              <button type="submit">Browse</button>
            </form>

            {dirError && <p className="error-msg">{dirError}</p>}

            {dirEntries.length > 0 && (
              <div className="card">
                <h3>Contents of "{dirPath}"</h3>
                <ul className="file-list">
                  {dirEntries.map((entry, i) => (
                    <li key={i}>{entry}</li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        )}

        {currentView === "voice" && (
          <div className="view">
            <h2>Voice Conversation</h2>
            <p>
              Tap the button to record your voice (auto-stops after 5 seconds).
              Your speech is transcribed, sent to the assistant, and the reply is
              spoken back.
            </p>

            <div className="card" style={{ display: "flex", flexDirection: "column", gap: 16, alignItems: "flex-start" }}>
              <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
                <Recording />
                <span className="result-msg">
                  Click once to start · click again (or wait 5s) to stop.
                </span>
              </div>
              <p className="subtitle">
                Tip: Add your API keys to a local <code>.env</code> file (see
                <code> .env.example</code>) before running.
              </p>
            </div>
          </div>
        )}

        {currentView === "notes" && (
          <div className="view">
            <h2>Notes Editor</h2>
            <p>Read and write text files using Tauri commands.</p>

            <div className="inline-form">
              <input
                type="text"
                value={notePath}
                onChange={(e) => setNotePath(e.target.value)}
                placeholder="File path (e.g. test-note.txt)..."
              />
              <button onClick={handleReadNote}>Load</button>
              <button onClick={handleSaveNote} className="save-btn">
                Save
              </button>
            </div>

            {noteMsg && (
              <p className={noteMsg.startsWith("Error") ? "error-msg" : "result-msg"}>
                {noteMsg}
              </p>
            )}

            <textarea
              className="note-editor"
              value={noteContent}
              onChange={(e) => setNoteContent(e.target.value)}
              placeholder="Start typing your note here..."
              rows={12}
            />
          </div>
        )}
      </main>
    </div>
  );
}

export default App;