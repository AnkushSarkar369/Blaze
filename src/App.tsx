import { useEffect, useMemo, useRef, useState, type WheelEvent } from "react";

function formatTime(seconds: number) {
  if (!Number.isFinite(seconds)) return "00:00.00";
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  return `${String(minutes).padStart(2, "0")}:${remainingSeconds.toFixed(2).padStart(5, "0")}`;
}

function App() {
  const fileInput = useRef<HTMLInputElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const timelineViewport = useRef<HTMLDivElement>(null);
  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  const [videoName, setVideoName] = useState("");
  const [duration, setDuration] = useState(0);
  const [currentTime, setCurrentTime] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [showKeyframes, setShowKeyframes] = useState(true);
  const [mode, setMode] = useState<"blaze" | "precise">("blaze");
  const [cuts, setCuts] = useState<number[]>([]);
  const [timelineWidth, setTimelineWidth] = useState(1000);

  const pixelsPerSecond = 72 * zoom;
  const contentWidth = Math.max(timelineWidth, duration * pixelsPerSecond + 80);

  // Temporary visual waveform for layout work. Real audio analysis will replace this.
  const waveform = useMemo(
    () => Array.from({ length: 520 }, (_, i) => {
      const envelope = 0.28 + 0.72 * Math.abs(Math.sin(i * 0.013) * Math.cos(i * 0.0037));
      const detail = 0.25 + 0.75 * Math.abs(Math.sin(i * 0.19) * Math.cos(i * 0.071));
      return 5 + envelope * detail * 31;
    }),
    [],
  );

  useEffect(() => {
    const element = timelineViewport.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setTimelineWidth(entry.contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    return () => {
      if (videoUrl) URL.revokeObjectURL(videoUrl);
    };
  }, [videoUrl]);

  function importVideo(file?: File) {
    if (!file) return;
    setVideoUrl((oldUrl) => {
      if (oldUrl) URL.revokeObjectURL(oldUrl);
      return URL.createObjectURL(file);
    });
    setVideoName(file.name);
    setDuration(0);
    setCurrentTime(0);
    setCuts([]);
    setZoom(1);
    if (timelineViewport.current) timelineViewport.current.scrollLeft = 0;
  }

  function handleTimelineWheel(event: WheelEvent<HTMLDivElement>) {
    if (event.ctrlKey) {
      event.preventDefault();
      const viewport = timelineViewport.current;
      if (!viewport) return;
      const rect = viewport.getBoundingClientRect();
      const cursorX = event.clientX - rect.left;
      const oldZoom = zoom;
      const nextZoom = Math.min(12, Math.max(0.25, oldZoom * (event.deltaY < 0 ? 1.12 : 1 / 1.12)));
      const contentX = viewport.scrollLeft + cursorX;
      const nextScroll = contentX * (nextZoom / oldZoom) - cursorX;
      setZoom(nextZoom);
      requestAnimationFrame(() => {
        if (timelineViewport.current) timelineViewport.current.scrollLeft = nextScroll;
      });
    } else if (event.altKey) {
      event.preventDefault();
      if (timelineViewport.current) timelineViewport.current.scrollLeft += event.deltaY || event.deltaX;
    }
  }

  function seekFromPointer(event: React.MouseEvent<HTMLDivElement>) {
    if (!duration || !timelineViewport.current) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const x = event.clientX - rect.left + timelineViewport.current.scrollLeft;
    const nextTime = Math.min(duration, Math.max(0, x / pixelsPerSecond));
    if (videoRef.current) videoRef.current.currentTime = nextTime;
    setCurrentTime(nextTime);
  }

  function addCut() {
    if (!duration) return;
    const keyframeInterval = 1.5; // Temporary design-time keyframe spacing; real values come from media analysis.\n    const nextCut = mode === "blaze" ? Math.round(currentTime / keyframeInterval) * keyframeInterval : currentTime;
    if (nextCut <= 0 || nextCut >= duration) return;
    setCuts((existing) => [...existing, nextCut].sort((a, b) => a - b));
  }

  const rulerStep = zoom >= 5 ? 0.5 : zoom >= 2 ? 1 : zoom >= 0.8 ? 5 : 10;
  const rulerTicks = duration
    ? Array.from({ length: Math.ceil(duration / rulerStep) + 1 }, (_, i) => i * rulerStep).filter((t) => t <= duration)
    : Array.from({ length: 9 }, (_, i) => i * 5);

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
              <div className="import-symbol"><svg viewBox="0 0 48 48" role="presentation"><path d="M24 32V8m0 0-9 9m9-9 9 9" /><path d="M9 29v9a2 2 0 0 0 2 2h26a2 2 0 0 0 2-2v-9" /></svg></div>
              <h2>Bring in your footage</h2>
              <p>Select a video file to begin.</p>
              <button className="primary-button" onClick={() => fileInput.current?.click()}>Import video</button>
              <input ref={fileInput} className="hidden-input" type="file" accept="video/*" onChange={(event) => importVideo(event.target.files?.[0])} />
            </div>
          )}
        </section>

        <section className="quadrant tools-quadrant">
          <div className="panel-heading"><span className="panel-label">TOOLS & MODES</span></div>
          <div className="tools-empty">
            <span>TOOLS WILL APPEAR HERE</span>
            <p>We'll design this panel after the timeline is established.</p>
          </div>
        </section>

        <section className="quadrant timeline-quadrant">
          <div className="panel-heading timeline-heading">
            <div className="timeline-title"><span className="panel-label">TIMELINE</span><span className="timeline-track-count">VIDEO + AUDIO</span></div>
            <span className="timecode">{formatTime(currentTime)} <span className="time-divider">/</span> {formatTime(duration)}</span>
          </div>
          <div className="timeline-toolbar">
            <div className="timeline-toolbar-left">
              <button className="timeline-action cut-action" onClick={addCut} disabled={!videoUrl} title="Add a cut at the playhead"><span className="cut-icon">⌁</span> CUT</button>
              <span className="toolbar-separator" />
              <button className={`timeline-action ${showKeyframes ? "active" : ""}`} onClick={() => setShowKeyframes(!showKeyframes)} aria-pressed={showKeyframes}>KEYFRAMES <span className="toggle-indicator" /></button>
              <span className="toolbar-separator" />
              <div className="mode-switch" role="group" aria-label="Cutting mode">
                <button className={mode === "blaze" ? "selected" : ""} onClick={() => setMode("blaze")}>BLAZE</button>
                <button className={mode === "precise" ? "selected" : ""} onClick={() => setMode("precise")}>PRECISE</button>
              </div>
            </div>
            <div className="timeline-toolbar-right">
              <span className="zoom-value">{Math.round(zoom * 100)}%</span>
              <button className="zoom-button" onClick={() => setZoom((z) => Math.max(0.25, z / 1.2))} aria-label="Zoom out">−</button>
              <button className="zoom-button" onClick={() => setZoom((z) => Math.min(12, z * 1.2))} aria-label="Zoom in">+</button>
            </div>
          </div>

          <div className="timeline-viewport" ref={timelineViewport} onWheel={handleTimelineWheel}>
            <div className="timeline-canvas" style={{ width: contentWidth }}>
              <div className="timeline-ruler" onClick={seekFromPointer}>
                {rulerTicks.map((tick) => <div className="ruler-tick" key={tick} style={{ left: tick * pixelsPerSecond }}><span>{formatTime(tick)}</span><i /></div>)}
              </div>
              <div className="track-row video-track" onClick={seekFromPointer}>
                <div className="track-label"><span className="track-icon">▣</span><span>VIDEO</span></div>
                <div className="track-lane">
                  {videoUrl ? <div className="clip-block" style={{ width: Math.max(4, duration * pixelsPerSecond) }}><span className="clip-name">{videoName}</span><div className="clip-thumbnails">{Array.from({ length: Math.min(36, Math.max(4, Math.ceil(duration / 2))) }, (_, i) => <span key={i} />)}</div></div> : <span className="track-empty-label">Import footage to populate the timeline</span>}
                </div>
              </div>
              <div className="track-row audio-track" onClick={seekFromPointer}>
                <div className="track-label"><span className="track-icon audio-icon">≋</span><span>AUDIO</span></div>
                <div className="track-lane">
                  {videoUrl ? <div className="waveform-clip" style={{ width: Math.max(4, duration * pixelsPerSecond) }}><div className="waveform-bars">{waveform.map((height, i) => <i key={i} style={{ height }} />)}</div></div> : <span className="track-empty-label">Audio waveform appears here</span>}
                </div>
              </div>
              {showKeyframes && videoUrl && <div className="keyframe-layer" style={{ width: contentWidth - 80, left: 80 }}>{Array.from({ length: Math.floor(duration / 1.5) + 1 }, (_, i) => <i key={i} style={{ left: i * 1.5 * pixelsPerSecond }} />)}</div>}
              {cuts.map((cut, i) => <div className="cut-marker" key={`${cut}-${i}`} style={{ left: 80 + cut * pixelsPerSecond }} title={`Cut at ${formatTime(cut)}`} />)}
              <div className="playhead" style={{ left: 80 + currentTime * pixelsPerSecond }}><div className="playhead-cap" /><div className="playhead-line" /></div>
            </div>
          </div>
          <div className="timeline-footer"><span>CTRL + SCROLL TO ZOOM</span><span>ALT + SCROLL TO PAN</span><span>{mode === "blaze" ? "KEYFRAME-ALIGNED CUTTING" : "FRAME-ACCURATE CUTTING"}</span></div>
        </section>
      </section>
    </main>
  );
}

export default App;
