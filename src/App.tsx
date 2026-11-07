import { useEffect, useMemo, useRef, useState, type WheelEvent } from "react";
import type React from "react";

type ToolTab = "blaze" | "precise" | "export";

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
  const [duration, setDuration] = useState(0);
  const [currentTime, setCurrentTime] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [activeTab, setActiveTab] = useState<ToolTab>("blaze");
  const [cuts, setCuts] = useState<number[]>([]);
  const [timelineWidth, setTimelineWidth] = useState(1000);
  const [previewWidth, setPreviewWidth] = useState(50);
  const [topHeight, setTopHeight] = useState(58);
  const [draggingDivider, setDraggingDivider] = useState<"vertical" | "horizontal" | null>(null);

  const mode = activeTab === "precise" ? "precise" : "blaze";
  const pixelsPerSecond = 72 * zoom;
  const contentWidth = Math.max(timelineWidth, duration * pixelsPerSecond + 40);
  const workspaceStyle = {
    gridTemplateColumns: `minmax(0, ${previewWidth}fr) 6px minmax(0, ${100 - previewWidth}fr)`,
    gridTemplateRows: `${topHeight}fr 6px ${100 - topHeight}fr`,
  } as React.CSSProperties;

  function startDividerDrag(kind: "vertical" | "horizontal", event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    setDraggingDivider(kind);
  }

  function moveDivider(event: React.PointerEvent<HTMLDivElement>) {
    if (!draggingDivider) return;
    const grid = event.currentTarget.closest(".workspace-grid");
    if (!grid) return;
    const rect = grid.getBoundingClientRect();
    if (draggingDivider === "vertical") {
      const percentage = ((event.clientX - rect.left) / rect.width) * 100;
      setPreviewWidth(Math.min(80, Math.max(20, percentage)));
    } else {
      const percentage = ((event.clientY - rect.top) / rect.height) * 100;
      setTopHeight(Math.min(82, Math.max(30, percentage)));
    }
  }

  function stopDividerDrag(event: React.PointerEvent<HTMLDivElement>) {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    setDraggingDivider(null);
  }

  // Placeholder waveform for interface design; actual audio decoding comes next.
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

  useEffect(() => () => {
    if (videoUrl) URL.revokeObjectURL(videoUrl);
  }, [videoUrl]);

  function importVideo(file?: File) {
    if (!file) return;
    setVideoUrl((oldUrl) => {
      if (oldUrl) URL.revokeObjectURL(oldUrl);
      return URL.createObjectURL(file);
    });
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
    // Temporary stand-in for detected keyframe timestamps. Real keyframes will come from media analysis.
    const detectedKeyframes = Array.from({ length: Math.floor(duration / 1.5) + 1 }, (_, i) => i * 1.5);
    const nearestKeyframe = detectedKeyframes.reduce((nearest, point) =>
      Math.abs(point - currentTime) < Math.abs(nearest - currentTime) ? point : nearest, detectedKeyframes[0] ?? 0);
    const nextCut = mode === "blaze" ? nearestKeyframe : currentTime;
    if (nextCut <= 0 || nextCut >= duration) return;
    setCuts((existing) => existing.some((cut) => Math.abs(cut - nextCut) < 0.001)
      ? existing : [...existing, nextCut].sort((a, b) => a - b));
  }

  const rulerStep = zoom >= 5 ? 0.5 : zoom >= 2 ? 1 : zoom >= 0.8 ? 5 : 10;
  const rulerTicks = duration
    ? Array.from({ length: Math.ceil(duration / rulerStep) + 1 }, (_, i) => i * rulerStep).filter((t) => t <= duration)
    : Array.from({ length: 9 }, (_, i) => i * 5);

  return (
    <main className="app-shell">
      <section className={`workspace-grid${draggingDivider ? " is-resizing" : ""}`} style={workspaceStyle}>
        <section className="quadrant preview-quadrant">
          {videoUrl ? (
            <video ref={videoRef} className="video-preview" src={videoUrl} controls playsInline
              onLoadedMetadata={(event) => setDuration(event.currentTarget.duration)}
              onTimeUpdate={(event) => setCurrentTime(event.currentTarget.currentTime)} />
          ) : (
            <div className="import-empty">
              <div className="import-symbol"><svg viewBox="0 0 48 48" role="presentation"><path d="M24 32V8m0 0-9 9m9-9 9 9" /><path d="M9 29v9a2 2 0 0 0 2 2h26a2 2 0 0 0 2-2v-9" /></svg></div>
              <h2>Bring in your footage</h2><p>Select a video file to begin.</p>
              <button className="primary-button" onClick={() => fileInput.current?.click()}>Import video</button>
              <input ref={fileInput} className="hidden-input" type="file" accept="video/*" onChange={(event) => importVideo(event.target.files?.[0])} />
            </div>
          )}
        </section>

        <div className="resize-divider vertical-divider" role="separator" aria-label="Resize video preview and tools panels" aria-orientation="vertical" onPointerDown={(event) => startDividerDrag("vertical", event)} onPointerMove={moveDivider} onPointerUp={stopDividerDrag} onPointerCancel={stopDividerDrag} />

        <section className="quadrant tools-quadrant">
          <nav className="tool-tabs" aria-label="Editing modes">
            <button className={activeTab === "blaze" ? "active" : ""} onClick={() => setActiveTab("blaze")}>BLAZE MODE</button>
            <button className={activeTab === "precise" ? "active" : ""} onClick={() => setActiveTab("precise")}>PRECISE MODE</button>
            <button className={activeTab === "export" ? "active" : ""} onClick={() => setActiveTab("export")}>EXPORT</button>
          </nav>
          <div className="tool-panel">
            {activeTab === "blaze" && <>
              <div className="tool-kicker">FAST · KEYFRAME-ALIGNED</div>
              <h2>Blaze Mode</h2>
              <p>Cuts automatically snap to detected video keyframes for stream-copy export where possible.</p>
              <button className="tool-primary" onClick={addCut} disabled={!videoUrl}>ADD CUT AT PLAYHEAD</button>
            </>}
            {activeTab === "precise" && <>
              <div className="tool-kicker">FRAME-ACCURATE · RE-ENCODE</div>
              <h2>Precise Mode</h2>
              <p>Place cuts at the exact playhead position. Export will re-encode where required.</p>
              <button className="tool-primary" onClick={addCut} disabled={!videoUrl}>ADD PRECISE CUT</button>
            </>}
            {activeTab === "export" && <>
              <div className="tool-kicker">OUTPUT</div>
              <h2>Export</h2>
              <p>{cuts.length} cut point{cuts.length === 1 ? "" : "s"} · {mode === "blaze" ? "Blaze" : "Precise"} mode</p>
              <button className="tool-primary" disabled={!videoUrl}>EXPORT VIDEO</button>
              <span className="tool-note">Export engine not connected yet.</span>
            </>}
          </div>
        </section>

        <div className="resize-divider horizontal-divider" role="separator" aria-label="Resize timeline and upper panels" aria-orientation="horizontal" onPointerDown={(event) => startDividerDrag("horizontal", event)} onPointerMove={moveDivider} onPointerUp={stopDividerDrag} onPointerCancel={stopDividerDrag} />

        <section className="quadrant timeline-quadrant">
          <div className="timeline-viewport" ref={timelineViewport} onWheel={handleTimelineWheel}>
            <div className="timeline-canvas" style={{ width: contentWidth }}>
              {videoUrl && <div className="timeline-ruler" onClick={seekFromPointer}>
                {rulerTicks.map((tick) => <div className="ruler-tick" key={tick} style={{ left: tick * pixelsPerSecond }}><span>{formatTime(tick)}</span><i /></div>)}
              </div>}
              <div className="waveform-track" onClick={seekFromPointer}>
                {videoUrl ? <div className="waveform-clip" style={{ width: Math.max(4, duration * pixelsPerSecond) }}>
                  <div className="waveform-bars">{waveform.map((height, i) => <i key={i} style={{ height }} />)}</div>
                </div> : null}
              </div>
              {mode === "blaze" && videoUrl && <div className="keyframe-layer" style={{ width: contentWidth }}>{Array.from({ length: Math.floor(duration / 1.5) + 1 }, (_, i) =>
                <i key={i} style={{ left: i * 1.5 * pixelsPerSecond }} />)}</div>}
              {cuts.map((cut, i) => <div className="cut-marker" key={`${cut}-${i}`} style={{ left: cut * pixelsPerSecond }} title={`Cut at ${formatTime(cut)}`} />)}
              <div className="playhead" style={{ left: currentTime * pixelsPerSecond }}><div className="playhead-cap" /><div className="playhead-line" /></div>
            </div>
          </div>
        </section>
      </section>
    </main>
  );
}

export default App;
