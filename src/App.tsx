import { useRef, useState } from "react";

function formatTime(seconds: number) {
  if (!Number.isFinite(seconds)) return "00:00";
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = Math.floor(seconds % 60);
  return `${String(minutes).padStart(2, "0")}:${String(remainingSeconds).padStart(2, "0")}`;
}

function App() {
  const fileInput = useRef<HTMLInputElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  const [videoName, setVideoName] = useState("");
  const [duration, setDuration] = useState(0);
  const [currentTime, setCurrentTime] = useState(0);

  function importVideo(file?: File) {
    if (!file) return;
    if (videoUrl) URL.revokeObjectURL(videoUrl);
    setVideoUrl(URL.createObjectURL(file));
    setVideoName(file.name);
    setDuration(0);
    setCurrentTime(0);
  }

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand">
          <div className="brand-mark">B</div>
          <span>blaze</span>
          <span className="version">0.1.0</span>
        </div>
        <div className="topbar-right">
          <span className="status-dot" />
          <span className="status-label">LOCAL WORKSPACE</span>
        </div>
      </header>

      <section className="workspace-grid">
        <section className="quadrant preview-quadrant">
          <div className="panel-heading">
            <span className="panel-label">VIDEO PREVIEW</span>
            {videoName && <span className="file-name" title={videoName}>{videoName}</span>}
          </div>

          {videoUrl ? (
            <video
              ref={videoRef}
              className="video-preview"
              src={videoUrl}
              controls
              playsInline
              onLoadedMetadata={(event) => setDuration(event.currentTarget.duration)}
              onTimeUpdate={(event) => setCurrentTime(event.currentTarget.currentTime)}
            />
          ) : (
            <div className="import-empty">
              <div className="import-symbol">
                <svg viewBox="0 0 48 48" role="presentation">
                  <path d="M24 32V8m0 0-9 9m9-9 9 9" />
                  <path d="M9 29v9a2 2 0 0 0 2 2h26a2 2 0 0 0 2-2v-9" />
                </svg>
              </div>
              <h2>Bring in your footage</h2>
              <p>Select a video file to begin.</p>
              <button className="primary-button" onClick={() => fileInput.current?.click()}>
                Import video
              </button>
              <input
                ref={fileInput}
                className="hidden-input"
                type="file"
                accept="video/*"
                onChange={(event) => importVideo(event.target.files?.[0])}
              />
            </div>
          )}
        </section>

        <section className="quadrant tools-quadrant">
          <div className="panel-heading">
            <span className="panel-label">TOOLS & MODES</span>
          </div>
          <div className="tools-empty">
            <span>TOOLS WILL APPEAR HERE</span>
            <p>Import a video to get started. Editing controls and modes will live in this panel.</p>
          </div>
        </section>

        <section className="quadrant timeline-quadrant">
          <div className="panel-heading timeline-heading">
            <span className="panel-label">TIMELINE</span>
            <span className="timecode">{formatTime(currentTime)} <span className="time-divider">/</span> {formatTime(duration)}</span>
          </div>
          <div className={`timeline-content ${videoUrl ? "has-video" : ""}`}>
            <div className="timeline-ruler">
              <span>00:00</span>
              <span>{formatTime(duration / 4)}</span>
              <span>{formatTime(duration / 2)}</span>
              <span>{formatTime((duration * 3) / 4)}</span>
              <span>{formatTime(duration)}</span>
            </div>
            <input
              className="timeline-scrubber"
              type="range"
              min="0"
              max={duration || 1}
              step="0.01"
              value={Math.min(currentTime, duration || 1)}
              disabled={!videoUrl || !duration}
              aria-label="Seek through video"
              onChange={(event) => {
                const nextTime = Number(event.target.value);
                if (videoRef.current) videoRef.current.currentTime = nextTime;
                setCurrentTime(nextTime);
              }}
            />
            {!videoUrl && <div className="timeline-placeholder">Import a video to populate the timeline.</div>}
          </div>
        </section>
      </section>
    </main>
  );
}

export default App;
