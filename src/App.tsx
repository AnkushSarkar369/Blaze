import { useEffect, useRef, useState, type WheelEvent } from "react";
import type React from "react";
import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type ToolTab = "blaze" | "precise" | "export";

interface MediaInfo {
  duration: number;
  width: number | null;
  height: number | null;
  fps: number | null;
  video_codec: string | null;
  audio_codec: string | null;
  has_audio: boolean;
}

function formatTime(seconds: number) {
  if (!Number.isFinite(seconds)) return "00:00.00";
  const safe = Math.max(0, seconds);
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  const remainingSeconds = safe % 60;
  if (hours > 0) {
    return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${remainingSeconds.toFixed(2).padStart(5, "0")}`;
  }
  return `${String(minutes).padStart(2, "0")}:${remainingSeconds.toFixed(2).padStart(5, "0")}`;
}

function App() {
  const videoRef = useRef<HTMLVideoElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const browserObjectUrl = useRef<string | null>(null);
  const timelineViewport = useRef<HTMLDivElement>(null);
  const loadToken = useRef(0);

  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  const [mediaPath, setMediaPath] = useState<string | null>(null);
  const [mediaInfo, setMediaInfo] = useState<MediaInfo | null>(null);
  const [duration, setDuration] = useState(0);
  const [currentTime, setCurrentTime] = useState(0);
  const [waveform, setWaveform] = useState<number[]>([]);
  const [keyframes, setKeyframes] = useState<number[]>([]);
  const [analysisLoading, setAnalysisLoading] = useState(false);
  const [analysisError, setAnalysisError] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const [activeTab, setActiveTab] = useState<ToolTab>("blaze");
  const [editingMode, setEditingMode] = useState<"blaze" | "precise">("blaze");
  const [cuts, setCuts] = useState<number[]>([]);
  const [timelineWidth, setTimelineWidth] = useState(1000);
  const [previewWidth, setPreviewWidth] = useState(50);
  const [topHeight, setTopHeight] = useState(58);
  const [draggingDivider, setDraggingDivider] = useState<"vertical" | "horizontal" | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [volume, setVolume] = useState(1);

  const mode = editingMode;
  const pixelsPerSecond = 72 * zoom;
  const contentWidth = Math.max(timelineWidth, duration * pixelsPerSecond + 40);
  const mediaName = mediaPath?.split(/[\\/]/).pop() ?? "";
  const workspaceStyle = {
    gridTemplateColumns: `minmax(0, ${previewWidth}fr) 6px minmax(0, ${100 - previewWidth}fr)`,
    gridTemplateRows: `${topHeight}fr 6px ${100 - topHeight}fr`,
  } as React.CSSProperties;

  useEffect(() => {
    const element = timelineViewport.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setTimelineWidth(entry.contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (videoRef.current) videoRef.current.volume = volume;
  }, [volume, videoUrl]);

  useEffect(() => () => {
    if (browserObjectUrl.current) URL.revokeObjectURL(browserObjectUrl.current);
  }, []);

  useEffect(() => {
    if (!isPlaying || !timelineViewport.current) return;
    const viewport = timelineViewport.current;
    const playheadX = currentTime * pixelsPerSecond;
    const visibleRight = viewport.scrollLeft + viewport.clientWidth;
    if (playheadX > visibleRight - 64) {
      viewport.scrollLeft = Math.max(0, playheadX - viewport.clientWidth * 0.72);
    }
  }, [currentTime, isPlaying, pixelsPerSecond]);

  useEffect(() => {
    function handleKeyboard(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null;
      if (event.code !== "Space" || target?.tagName === "INPUT" || target?.tagName === "TEXTAREA") return;
      if (!videoUrl) return;
      event.preventDefault();
      togglePlayback();
    }
    window.addEventListener("keydown", handleKeyboard);
    return () => window.removeEventListener("keydown", handleKeyboard);
  }, [videoUrl]);

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
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    setDraggingDivider(null);
  }

  function importBrowserVideo(file?: File) {
    if (!file) return;
    if (browserObjectUrl.current) URL.revokeObjectURL(browserObjectUrl.current);
    const objectUrl = URL.createObjectURL(file);
    browserObjectUrl.current = objectUrl;
    setVideoUrl(objectUrl);
    setMediaPath(file.name);
    setMediaInfo(null);
    setDuration(0);
    setCurrentTime(0);
    setWaveform([]);
    setKeyframes([]);
    setCuts([]);
    setZoom(1);
    setAnalysisError("Browser preview only. FFmpeg analysis is available in the desktop app.");
  }

  async function importVideo() {
    if (!isTauri()) {
      fileInput.current?.click();
      return;
    }
    let token = loadToken.current;
    try {
      const selected = await open({
        title: "Import footage",
        multiple: false,
        filters: [{
          name: "Video files",
          extensions: ["mp4", "mov", "mkv", "webm", "avi", "m4v", "mts", "m2ts", "ts", "3gp", "wmv", "mpg", "mpeg"],
        }],
      });
      if (typeof selected !== "string") return;

      token = ++loadToken.current;
      videoRef.current?.pause();
      setIsPlaying(false);
      setMediaPath(selected);
      setVideoUrl(convertFileSrc(selected));
      setMediaInfo(null);
      setDuration(0);
      setCurrentTime(0);
      setWaveform([]);
      setKeyframes([]);
      setCuts([]);
      setZoom(1);
      setAnalysisError(null);
      setAnalysisLoading(true);
      if (timelineViewport.current) timelineViewport.current.scrollLeft = 0;

      const info = await invoke<MediaInfo>("probe_media", { path: selected });
      if (token !== loadToken.current) return;
      setMediaInfo(info);
      setDuration(info.duration);

      const [waveResult, keyframeResult] = await Promise.allSettled([
        info.has_audio
          ? invoke<number[]>("generate_waveform", {
              path: selected,
              duration: info.duration,
              bars: 1800,
            })
          : Promise.resolve([]),
        invoke<number[]>("detect_keyframes", { path: selected }),
      ]);

      if (token !== loadToken.current) return;

      if (waveResult.status === "fulfilled") {
        setWaveform(waveResult.value);
      } else {
        setAnalysisError(String(waveResult.reason));
      }

      if (keyframeResult.status === "fulfilled") {
        setKeyframes(keyframeResult.value);
      } else {
        setAnalysisError((current) => current
          ? `${current} · Keyframe scan: ${String(keyframeResult.reason)}`
          : `Keyframe scan: ${String(keyframeResult.reason)}`);
      }
    } catch (error) {
      if (token === loadToken.current) setAnalysisError(String(error));
    } finally {
      if (token === loadToken.current) setAnalysisLoading(false);
    }
  }

  function togglePlayback() {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) {
      void video.play().catch((error) => setAnalysisError(`Playback failed: ${String(error)}`));
    } else {
      video.pause();
    }
  }

  function seekVideo(nextTime: number) {
    const safeTime = Math.min(duration, Math.max(0, nextTime));
    if (videoRef.current) videoRef.current.currentTime = safeTime;
    setCurrentTime(safeTime);
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
      if (timelineViewport.current) {
        timelineViewport.current.scrollLeft += event.deltaY || event.deltaX;
      }
    }
  }

  function seekFromPointer(event: React.MouseEvent<HTMLDivElement>) {
    const viewport = timelineViewport.current;
    if (!duration || !viewport) return;
    const rect = viewport.getBoundingClientRect();
    const x = event.clientX - rect.left + viewport.scrollLeft;
    seekVideo(x / pixelsPerSecond);
  }

  function addCut() {
    if (!duration) return;
    let nextCut = currentTime;

    if (mode === "blaze" && keyframes.length > 0) {
      nextCut = keyframes.reduce((nearest, point) =>
        Math.abs(point - currentTime) < Math.abs(nearest - currentTime) ? point : nearest,
      keyframes[0]);
    }

    if (nextCut <= 0 || nextCut >= duration) return;
    setCuts((existing) => existing.some((cut) => Math.abs(cut - nextCut) < 0.001)
      ? existing
      : [...existing, nextCut].sort((a, b) => a - b));
  }

  const baseRulerStep = zoom >= 6 ? 0.5 : zoom >= 3 ? 1 : zoom >= 1.2 ? 2 : zoom >= 0.6 ? 5 : 10;
  const rulerStep = [0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800]
    .find((step) => step >= Math.max(baseRulerStep, duration / 300)) ?? 3600;
  const rulerTicks = duration
    ? Array.from({ length: Math.ceil(duration / rulerStep) + 1 }, (_, index) => index * rulerStep)
        .filter((tick) => tick <= duration)
    : Array.from({ length: 9 }, (_, index) => index * 5);

  return (
    <main className="app-shell">
      <input
        ref={fileInput}
        className="hidden-input"
        type="file"
        accept="video/*"
        onChange={(event) => {
          importBrowserVideo(event.target.files?.[0]);
          event.target.value = "";
        }}
      />
      <section className={`workspace-grid${draggingDivider ? " is-resizing" : ""}`} style={workspaceStyle}>
        <section className="quadrant preview-quadrant">
          <div className="preview-stage">
            {videoUrl ? (
              <video
                ref={videoRef}
                className="video-preview"
                src={videoUrl}
                playsInline
                preload="metadata"
                onLoadedMetadata={(event) => {
                  if (!duration && Number.isFinite(event.currentTarget.duration)) {
                    setDuration(event.currentTarget.duration);
                  }
                }}
                onTimeUpdate={(event) => setCurrentTime(event.currentTarget.currentTime)}
                onPlay={() => setIsPlaying(true)}
                onPause={() => setIsPlaying(false)}
                onEnded={() => setIsPlaying(false)}
                onClick={togglePlayback}
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
                <button className="primary-button" onClick={() => void importVideo()}>Import video</button>
              </div>
            )}
          </div>

          {videoUrl && (
            <div className="player-controls">
              <button className="player-button" onClick={togglePlayback} aria-label={isPlaying ? "Pause" : "Play"}>
                {isPlaying ? "Ⅱ" : "▶"}
              </button>
              <span className="player-time">{formatTime(currentTime)}</span>
              <input
                className="player-seek"
                aria-label="Seek video"
                type="range"
                min="0"
                max={Math.max(duration, 0.01)}
                step="0.01"
                value={Math.min(currentTime, duration)}
                onChange={(event) => seekVideo(Number(event.target.value))}
              />
              <span className="player-time">{formatTime(duration)}</span>
              <button
                className="player-button secondary-player-button"
                onClick={() => setVolume((value) => value > 0 ? 0 : 1)}
                aria-label={volume > 0 ? "Mute" : "Unmute"}
              >
                {volume > 0 ? "VOL" : "MUTE"}
              </button>
              <input
                className="player-volume"
                aria-label="Volume"
                type="range"
                min="0"
                max="1"
                step="0.05"
                value={volume}
                onChange={(event) => setVolume(Number(event.target.value))}
              />
              <button
                className="player-button secondary-player-button"
                onClick={() => void videoRef.current?.parentElement?.requestFullscreen?.()}
                aria-label="Fullscreen"
              >
                ⛶
              </button>
            </div>
          )}

          {mediaInfo && (
            <div className="media-meta">
              <span>{mediaName}</span>
              <span>
                {mediaInfo.width && mediaInfo.height ? `${mediaInfo.width}×${mediaInfo.height}` : "Video"}
                {mediaInfo.fps ? ` · ${mediaInfo.fps.toFixed(2)} fps` : ""}
                {mediaInfo.video_codec ? ` · ${mediaInfo.video_codec}` : ""}
              </span>
              <button onClick={() => void importVideo()}>REPLACE</button>
            </div>
          )}
        </section>

        <div
          className="resize-divider vertical-divider"
          role="separator"
          aria-label="Resize video preview and tools panels"
          aria-orientation="vertical"
          onPointerDown={(event) => startDividerDrag("vertical", event)}
          onPointerMove={moveDivider}
          onPointerUp={stopDividerDrag}
          onPointerCancel={stopDividerDrag}
        />

        <section className="quadrant tools-quadrant">
          <nav className="tool-tabs" aria-label="Editing modes">
            <button className={activeTab === "blaze" ? "active" : ""} onClick={() => { setActiveTab("blaze"); setEditingMode("blaze"); }}>BLAZE MODE</button>
            <button className={activeTab === "precise" ? "active" : ""} onClick={() => { setActiveTab("precise"); setEditingMode("precise"); }}>PRECISE MODE</button>
            <button className={activeTab === "export" ? "active" : ""} onClick={() => setActiveTab("export")}>EXPORT</button>
          </nav>
          <div className="tool-panel">
            {activeTab === "blaze" && <>
              <div className="tool-kicker">FAST · KEYFRAME-ALIGNED</div>
              <h2>Blaze Mode</h2>
              <p>Cuts snap to real keyframes detected in the source video.</p>
              <button className="tool-primary" onClick={addCut} disabled={!videoUrl}>ADD CUT AT PLAYHEAD</button>
              {videoUrl && keyframes.length === 0 && <span className="tool-note">Keyframe analysis is unavailable.</span>}
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
            {analysisLoading && <span className="tool-note">Analysing media with FFmpeg…</span>}
            {analysisError && <span className="tool-error">{analysisError}</span>}
          </div>
        </section>

        <div
          className="resize-divider horizontal-divider"
          role="separator"
          aria-label="Resize timeline and upper panels"
          aria-orientation="horizontal"
          onPointerDown={(event) => startDividerDrag("horizontal", event)}
          onPointerMove={moveDivider}
          onPointerUp={stopDividerDrag}
          onPointerCancel={stopDividerDrag}
        />

        <section className="quadrant timeline-quadrant">
          <div className="timeline-toolbar">
            <span className="timeline-title">TIMELINE</span>
            <span className="timeline-meta">
              {mediaInfo ? `${formatTime(duration)} · ${keyframes.length.toLocaleString()} keyframes` : "No media loaded"}
            </span>
            <span className="timeline-help">CTRL + SCROLL TO ZOOM · ALT + SCROLL TO PAN</span>
            <span className="timeline-zoom">{Math.round(zoom * 100)}%</span>
          </div>

          <div className="timeline-viewport" ref={timelineViewport} onWheel={handleTimelineWheel}>
            <div className="timeline-canvas" style={{ width: contentWidth }}>
              {videoUrl && (
                <div className="timeline-ruler" onClick={seekFromPointer}>
                  {rulerTicks.map((tick) => (
                    <div className="ruler-tick" key={tick} style={{ left: tick * pixelsPerSecond }}>
                      <span>{formatTime(tick)}</span><i />
                    </div>
                  ))}
                </div>
              )}

              <div className="keyframe-track" onClick={seekFromPointer}>
                <span className="track-label">VIDEO</span>
                <div className="keyframe-lane">
                  {keyframes.map((time, index) => (
                    <i key={index} style={{ left: time * pixelsPerSecond }} title={formatTime(time)} />
                  ))}
                  {!keyframes.length && videoUrl && <span className="track-empty">Keyframes will appear here</span>}
                </div>
              </div>

              <div className="waveform-track" onClick={seekFromPointer}>
                <span className="track-label">AUDIO</span>
                {videoUrl ? (
                  <div className="waveform-clip" style={{ width: Math.max(4, duration * pixelsPerSecond) }}>
                    {waveform.length > 0 ? (
                      <div className="waveform-bars">
                        {waveform.map((peak, index) => (
                          <i key={index} style={{ height: `${Math.max(2, peak * 64)}px` }} />
                        ))}
                      </div>
                    ) : (
                      <span className="track-empty">
                        {mediaInfo && !mediaInfo.has_audio
                          ? "No audio stream"
                          : analysisLoading ? "Generating waveform…" : "Waveform unavailable"}
                      </span>
                    )}
                  </div>
                ) : (
                  <span className="track-empty">Import footage to view its waveform</span>
                )}
              </div>

              {cuts.map((cut) => (
                <div className="cut-marker" key={cut} style={{ left: cut * pixelsPerSecond }} title={`Cut at ${formatTime(cut)}`} />
              ))}
              <div className="playhead" style={{ left: currentTime * pixelsPerSecond }}>
                <div className="playhead-cap" />
                <div className="playhead-line" />
              </div>
            </div>
          </div>
        </section>
      </section>
    </main>
  );
}

export default App;
