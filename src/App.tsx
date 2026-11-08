import { useEffect, useRef, useState } from "react";
import type React from "react";
import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Player } from "./player";
import { drawWaveform, parseWaveform, type WaveformData } from "./waveform";

type ToolTab = "blaze" | "precise" | "export";

interface MediaInfo {
  duration: number;
  width: number | null;
  height: number | null;
  fps: number | null;
  video_codec: string | null;
  audio_codec: string | null;
  audio_channels: number | null;
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
  const previewCanvas = useRef<HTMLCanvasElement>(null);
  const audioElement = useRef<HTMLAudioElement>(null);
  const player = useRef<Player | null>(null);
  const timelineViewport = useRef<HTMLDivElement>(null);
  const waveformCanvas = useRef<HTMLCanvasElement>(null);
  const loadToken = useRef(0);

  const [mediaPath, setMediaPath] = useState<string | null>(null);
  const [mediaInfo, setMediaInfo] = useState<MediaInfo | null>(null);
  const [duration, setDuration] = useState(0);
  const [currentTime, setCurrentTime] = useState(0);
  const [waveform, setWaveform] = useState<WaveformData | null>(null);
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
  const [canPlay, setCanPlay] = useState(false);

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
    const canvas = previewCanvas.current;
    const audio = audioElement.current;
    if (!canvas || !audio) return;
    const instance = new Player(canvas, audio, {
      onTime: setCurrentTime,
      onPlayingChange: setIsPlaying,
      onError: (message) => setAnalysisError(message),
      onReadyChange: setCanPlay,
    });
    player.current = instance;
    return () => {
      instance.dispose();
      player.current = null;
    };
  }, []);

  useEffect(() => {
    player.current?.setVolume(volume);
  }, [volume]);

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
    const canvas = waveformCanvas.current;
    const viewport = timelineViewport.current;
    if (!canvas || !viewport || !waveform) return;
    let frame = 0;
    const render = () => {
      frame = 0;
      drawWaveform(canvas, waveform, {
        scrollLeft: viewport.scrollLeft,
        pixelsPerSecond,
      });
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(render);
    };
    render();
    viewport.addEventListener("scroll", schedule, { passive: true });
    const observer = new ResizeObserver(schedule);
    observer.observe(canvas);
    return () => {
      viewport.removeEventListener("scroll", schedule);
      observer.disconnect();
      if (frame) cancelAnimationFrame(frame);
    };
  }, [waveform, pixelsPerSecond, timelineWidth]);

  // React registers onWheel as a passive listener, which makes preventDefault()
  // a no-op. Attach natively so Ctrl+scroll zooms the timeline, not the page.
  useEffect(() => {
    const viewport = timelineViewport.current;
    if (!viewport) return;
    viewport.addEventListener("wheel", handleTimelineWheel, { passive: false });
    return () => viewport.removeEventListener("wheel", handleTimelineWheel);
  }, [zoom]);

  useEffect(() => {
    function handleKeyboard(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null;
      if (event.code !== "Space" || target?.tagName === "INPUT" || target?.tagName === "TEXTAREA") return;
      if (!mediaPath) return;
      event.preventDefault();
      togglePlayback();
    }
    window.addEventListener("keydown", handleKeyboard);
    return () => window.removeEventListener("keydown", handleKeyboard);
  }, [mediaPath]);

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

  async function importVideo() {
    if (!isTauri()) {
      setAnalysisError("Importing needs the Blaze desktop app. A plain browser cannot run FFmpeg.");
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
      player.current?.pause();
      setIsPlaying(false);
      setCanPlay(false);
      setMediaPath(selected);
      setMediaInfo(null);
      setDuration(0);
      setCurrentTime(0);
      setWaveform(null);
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
      player.current?.load({
        path: selected,
        duration: info.duration,
        fps: info.fps,
        hasAudio: info.has_audio,
      });

      const [waveResult, keyframeResult, audioResult] = await Promise.allSettled([
        info.has_audio
          ? invoke<ArrayBuffer>("generate_waveform", {
              path: selected,
              channels: info.audio_channels ?? 2,
            }).then(parseWaveform)
          : Promise.resolve(null),
        invoke<number[]>("detect_keyframes", { path: selected }),
        info.has_audio
          ? invoke<string>("prepare_audio", { path: selected })
          : Promise.resolve(null),
      ]);

      if (token !== loadToken.current) return;

      if (audioResult.status === "fulfilled") {
        if (audioResult.value) player.current?.setAudioSource(convertFileSrc(audioResult.value));
      } else {
        player.current?.disableAudio(`Audio could not be prepared (${String(audioResult.reason)}). Playing without sound.`);
      }

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
    player.current?.toggle();
  }

  function seekVideo(nextTime: number) {
    const safeTime = Math.min(duration, Math.max(0, nextTime));
    player.current?.seek(safeTime);
    setCurrentTime(safeTime);
  }

  function handleTimelineWheel(event: WheelEvent) {
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

  return (
    <main className="app-shell">
      <audio ref={audioElement} hidden />
      <section className={`workspace-grid${draggingDivider ? " is-resizing" : ""}`} style={workspaceStyle}>
        <section className="quadrant preview-quadrant">
          <div className="preview-stage">
            <canvas
              ref={previewCanvas}
              className="video-preview"
              hidden={!mediaPath}
              onClick={togglePlayback}
            />
            {!mediaPath && (
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

          {mediaPath && (
            <div className="player-controls">
              <button
                className="player-button"
                onClick={togglePlayback}
                disabled={!isPlaying && !canPlay}
                title={!isPlaying && !canPlay ? "Preparing audio…" : undefined}
                aria-label={isPlaying ? "Pause" : "Play"}
              >
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
                onClick={() => void previewCanvas.current?.parentElement?.requestFullscreen?.()}
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
              <button className="tool-primary" onClick={addCut} disabled={!mediaPath}>ADD CUT AT PLAYHEAD</button>
              {mediaPath && keyframes.length === 0 && <span className="tool-note">Keyframe analysis is unavailable.</span>}
            </>}
            {activeTab === "precise" && <>
              <div className="tool-kicker">FRAME-ACCURATE · RE-ENCODE</div>
              <h2>Precise Mode</h2>
              <p>Place cuts at the exact playhead position. Export will re-encode where required.</p>
              <button className="tool-primary" onClick={addCut} disabled={!mediaPath}>ADD PRECISE CUT</button>
            </>}
            {activeTab === "export" && <>
              <div className="tool-kicker">OUTPUT</div>
              <h2>Export</h2>
              <p>{cuts.length} cut point{cuts.length === 1 ? "" : "s"} · {mode === "blaze" ? "Blaze" : "Precise"} mode</p>
              <button className="tool-primary" disabled={!mediaPath}>EXPORT VIDEO</button>
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

          <div className="timeline-viewport" ref={timelineViewport}>
            <div className="timeline-canvas" style={{ width: contentWidth }}>
              <div className="waveform-track" onClick={seekFromPointer}>
                {waveform ? (
                  <canvas
                    ref={waveformCanvas}
                    className="waveform-canvas"
                    style={{ width: timelineWidth }}
                  />
                ) : (
                  <span className="track-empty">
                    {!mediaPath
                      ? "Import footage to view its waveform"
                      : mediaInfo && !mediaInfo.has_audio
                        ? "No audio stream"
                        : analysisLoading ? "Generating waveform…" : "Waveform unavailable"}
                  </span>
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
