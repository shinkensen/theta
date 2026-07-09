import { useEffect, useRef } from "react";
import { useVoiceAssistant, type AssistantStatus } from "./hooks/useVoiceAssistant";
import "./App.css";

const STATUS_COPY: Record<AssistantStatus, string> = {
  standby: "STANDBY",
  listening: "LISTENING",
  thinking: "THINKING",
  speaking: "RESPONDING",
  error: "SIGNAL LOST",
};

const TICK_COUNT = 36;

function Dial({
  status,
  onPress,
}: {
  status: AssistantStatus;
  onPress: () => void;
}) {
  const ticks = Array.from({ length: TICK_COUNT }, (_, i) => i);

  return (
    <button
      className={`dial dial--${status}`}
      onClick={onPress}
      aria-pressed={status === "listening"}
      aria-label={
        status === "listening" ? "Stop listening" : "Start listening"
      }
      disabled={status === "thinking" || status === "speaking"}
    >
      <svg className="dial__ring" viewBox="0 0 200 200" aria-hidden="true">
        {ticks.map((i) => {
          const angle = (i / TICK_COUNT) * 360;
          return (
            <line
              key={i}
              className="dial__tick"
              style={{ animationDelay: `${(i / TICK_COUNT) * 1.6}s` }}
              x1="100"
              y1="14"
              x2="100"
              y2="26"
              transform={`rotate(${angle} 100 100)`}
            />
          );
        })}
      </svg>
      <span className="dial__core">
        <svg viewBox="0 0 24 24" className="dial__mic" aria-hidden="true">
          <rect x="9" y="2" width="6" height="12" rx="3" fill="currentColor" />
          <path
            d="M5 11a7 7 0 0 0 14 0"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
          />
          <line
            x1="12"
            y1="18"
            x2="12"
            y2="22"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
          />
        </svg>
      </span>
    </button>
  );
}

function App() {
  const { status, partial, transcript, errorMessage, toggleListening } =
    useVoiceAssistant();
  const logEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [transcript, partial]);

  return (
    <div className="app">
      <header className="statusbar">
        <div className="statusbar__mark">
          <span className="statusbar__glyph">θ</span>
          <span>Theta</span>
        </div>
        <div className={`statusbar__state statusbar__state--${status}`}>
          <span className="statusbar__dot" />
          {STATUS_COPY[status]}
        </div>
      </header>

      <main className="log">
        {transcript.length === 0 && !partial && (
          <div className="log__empty">
            No signal yet. Press the dial and say something.
          </div>
        )}

        {transcript.map((entry) => (
          <div key={entry.id} className={`entry entry--${entry.role}`}>
            <span className="entry__label">
              {entry.role === "user" ? "YOU" : "THETA"}
            </span>
            <p className="entry__text">{entry.text}</p>
          </div>
        ))}

        {partial && (
          <div className="entry entry--user entry--partial">
            <span className="entry__label">YOU</span>
            <p className="entry__text">{partial}</p>
          </div>
        )}

        <div ref={logEndRef} />
      </main>

      {status === "error" && errorMessage && (
        <div className="banner" role="alert">
          {errorMessage} — press the dial to retry.
        </div>
      )}

      <footer className="console">
        <Dial status={status} onPress={toggleListening} />
      </footer>
    </div>
  );
}

export default App;