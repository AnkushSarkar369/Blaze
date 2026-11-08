// FFmpeg-backed preview player.
//
// Video: the backend streams numbered JPEG frames (src-tauri/src/player.rs) that
// are drawn on a canvas. Frame N belongs to `streamStart + N / fps`, so the
// frame index is also its timestamp.
// Audio: a WAV decoded by FFmpeg plays through an <audio> element, which is the
// master clock whenever the file has audio. Video frames chase that clock and
// late ones are dropped, so the picture can never drift away from the sound.
// Nothing here depends on the webview being able to decode the source codec.

import { Channel, invoke } from "@tauri-apps/api/core";

export interface PlayerMedia {
  path: string;
  duration: number;
  fps: number | null;
  hasAudio: boolean;
}

export interface PlayerEvents {
  onTime(seconds: number): void;
  onPlayingChange(playing: boolean): void;
  onError(message: string): void;
  /** Fires when `canPlay` changes (e.g. the audio finished preparing). */
  onReadyChange(canPlay: boolean): void;
}

const END_MARKER = 0xffffffff;
const ERROR_MARKER = 0xfffffffe;
/** Keeps the frame that contains the requested time (see player.rs). */
const FRAME_EPSILON = 0.0005;
/** Minimum gap between stream restarts while scrubbing during playback. */
const SEEK_THROTTLE_MS = 90;
/** How long the audio element gets to actually start before we give up on it. */
const AUDIO_START_TIMEOUT_MS = 4000;

interface QueuedFrame {
  index: number;
  bitmap: ImageBitmap;
}

export class Player {
  private readonly ctx: CanvasRenderingContext2D;

  private media: PlayerMedia | null = null;
  private fps = 30;
  private duration = 0;
  private position = 0;

  private playing = false;
  /** Playing, but waiting for the first frame before the clock starts. */
  private starting = false;
  private gateTime = 0;

  private audioReady = false;
  private audioFailed = false;
  private audioToken = 0;
  private audioLast = 0;
  private audioStamp = 0;
  private audioRunning = false;
  private wallAnchor = 0;
  private wallBase = 0;

  private generation = 0;
  private sessionPromise: Promise<number> | null = null;
  private streamStart = 0;
  private streamEnded = false;
  private queue: QueuedFrame[] = [];
  private pendingAcks = 0;

  private stillBusy = false;
  private stillPending: number | null = null;

  private seekTimer: number | null = null;
  private lastRestart = 0;
  private raf = 0;
  private disposed = false;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly audio: HTMLAudioElement,
    private readonly events: PlayerEvents,
  ) {
    const ctx = canvas.getContext("2d", { alpha: false });
    if (!ctx) throw new Error("Canvas 2D is not available.");
    this.ctx = ctx;
    audio.preload = "auto";
  }

  get isPlaying() {
    return this.playing;
  }

  get canPlay() {
    if (!this.media) return false;
    return !this.media.hasAudio || this.audioReady || this.audioFailed;
  }

  private get useAudio() {
    return !!this.media?.hasAudio && this.audioReady && !this.audioFailed;
  }

  // ---- loading ------------------------------------------------------------

  load(media: PlayerMedia) {
    this.halt();
    this.media = media;
    this.duration = media.duration;
    this.fps = Number.isFinite(media.fps) && media.fps! > 0 ? Math.min(60, Math.max(1, media.fps!)) : 30;
    this.position = 0;
    this.audioReady = false;
    this.audioFailed = false;
    this.audioToken++;
    this.audio.removeAttribute("src");
    this.audio.load();
    this.events.onTime(0);
    this.events.onReadyChange(this.canPlay);
    this.requestStill(0);
  }

  setAudioSource(url: string) {
    if (!this.media) return;
    const token = ++this.audioToken;
    this.audioReady = false;
    this.audioFailed = false;

    const cleanup = () => {
      this.audio.removeEventListener("loadedmetadata", onReady);
      this.audio.removeEventListener("error", onError);
    };
    const onReady = () => {
      if (token !== this.audioToken) return cleanup();
      cleanup();
      this.audioReady = true;
      this.audio.volume = this.volume;
      this.events.onReadyChange(this.canPlay);
    };
    const onError = () => {
      if (token !== this.audioToken) return cleanup();
      cleanup();
      this.disableAudio("The audio could not be played by this system's media framework. Playing without sound.");
    };
    this.audio.addEventListener("loadedmetadata", onReady);
    this.audio.addEventListener("error", onError);
    this.audio.src = url;
    this.audio.load();
  }

  /** Falls back to silent playback (audio decode or output failed). */
  disableAudio(message: string) {
    this.audioToken++;
    this.audioFailed = true;
    this.audioReady = false;
    if (this.playing) this.pause();
    this.events.onError(message);
    this.events.onReadyChange(this.canPlay);
  }

  private volume = 1;

  setVolume(value: number) {
    this.volume = Math.min(1, Math.max(0, value));
    this.audio.volume = this.volume;
  }

  // ---- transport ----------------------------------------------------------

  play() {
    if (!this.media || this.playing || !this.canPlay) return;
    if (this.position >= this.duration - 0.05) this.position = 0;
    this.playing = true;
    this.cancelSeekTimer();
    this.beginGate();
    this.beginStream(this.position, 0);
    this.startLoop();
    this.events.onPlayingChange(true);
    this.events.onTime(this.position);
  }

  pause() {
    if (!this.playing) return;
    const stoppedAt = this.clock();
    this.halt();
    this.position = Math.min(this.duration, Math.max(0, stoppedAt));
    this.events.onPlayingChange(false);
    this.events.onTime(this.position);
  }

  toggle() {
    if (this.playing) this.pause();
    else this.play();
  }

  seek(seconds: number) {
    if (!this.media) return;
    const target = Math.min(Math.max(0, this.duration - 0.001), Math.max(0, seconds));
    this.position = target;
    this.events.onTime(target);

    if (!this.playing) {
      this.requestStill(target);
      return;
    }

    // Playing: restart the stream at the new position, at most once per
    // SEEK_THROTTLE_MS so dragging the seek bar does not spawn a process per event.
    this.audio.pause();
    this.starting = true;
    this.gateTime = performance.now();
    // Drop the old stream now, so its frames cannot release the clock gate
    // while the throttled restart below is still pending.
    this.endStream();
    if (this.seekTimer !== null) return;
    const wait = Math.max(0, SEEK_THROTTLE_MS - (performance.now() - this.lastRestart));
    this.seekTimer = window.setTimeout(() => {
      this.seekTimer = null;
      if (!this.playing) return;
      this.beginGate();
      this.beginStream(this.position, 0);
    }, wait);
  }

  dispose() {
    this.disposed = true;
    this.halt();
    this.audioToken++;
    this.audio.removeAttribute("src");
    this.audio.load();
  }

  // ---- clock --------------------------------------------------------------

  private beginGate() {
    this.starting = true;
    this.gateTime = performance.now();
    this.audioRunning = false;
    this.audioLast = this.position;
    this.audio.pause();
  }

  /** The first frame is ready: start the clock and the sound together. */
  private releaseGate() {
    this.starting = false;
    const now = performance.now();
    this.gateTime = now;
    if (this.useAudio) {
      this.audioLast = this.position;
      this.audioStamp = now;
      this.audioRunning = false;
      try {
        this.audio.currentTime = this.position;
      } catch {
        // Not seekable yet; playback just starts from where it is.
      }
      void this.audio.play().catch(() => {
        this.disableAudio("The audio could not be started. Playing without sound.");
      });
    } else {
      this.wallAnchor = now;
      this.wallBase = this.position;
    }
  }

  private clock(): number {
    if (!this.playing || this.starting) return this.position;
    const now = performance.now();

    if (this.useAudio) {
      const reading = this.audio.currentTime;
      if (reading !== this.audioLast) {
        this.audioLast = reading;
        this.audioStamp = now;
        this.audioRunning = true;
      }
      if (!this.audioRunning) return this.position;
      if (this.audio.ended) return this.duration;
      // Audio clocks advance in coarse steps; interpolate between them.
      return this.audioLast + Math.min(0.1, (now - this.audioStamp) / 1000);
    }

    return this.wallBase + (now - this.wallAnchor) / 1000;
  }

  // ---- frame stream -------------------------------------------------------

  private beginStream(from: number, maxFrames: number) {
    if (!this.media) return;
    this.endStream();
    this.lastRestart = performance.now();
    const generation = ++this.generation;
    this.streamStart = Math.max(0, from - 1 / this.fps + FRAME_EPSILON);

    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (buffer) => void this.onMessage(generation, buffer);

    this.sessionPromise = invoke<number>("player_start", {
      path: this.media.path,
      start: this.streamStart,
      fps: this.fps,
      maxFrames,
      onFrame: channel,
    }).catch((error) => {
      if (generation === this.generation) this.fail(String(error));
      return -1;
    });
  }

  private endStream() {
    const pending = this.sessionPromise;
    this.sessionPromise = null;
    if (pending) {
      void pending.then((id) => {
        if (id >= 0) void invoke("player_stop", { session: id }).catch(() => {});
      });
    }
    this.generation++;
    for (const frame of this.queue) frame.bitmap.close();
    this.queue = [];
    this.pendingAcks = 0;
    this.streamEnded = false;
    this.stillBusy = false;
    this.stillPending = null;
  }

  private async onMessage(generation: number, buffer: ArrayBuffer) {
    if (generation !== this.generation || this.disposed) return;
    if (buffer.byteLength < 4) return;
    const tag = new DataView(buffer).getUint32(0, true);

    if (tag === END_MARKER) {
      this.streamEnded = true;
      if (!this.playing) this.finishStill();
      return;
    }
    if (tag === ERROR_MARKER) {
      this.fail(new TextDecoder().decode(new Uint8Array(buffer, 4)));
      return;
    }

    let bitmap: ImageBitmap;
    try {
      bitmap = await createImageBitmap(new Blob([new Uint8Array(buffer, 4)], { type: "image/jpeg" }));
    } catch {
      // A damaged frame is skipped; return its credit so the stream keeps flowing.
      if (generation === this.generation) {
        this.pendingAcks += 1;
        this.flushAcks();
      }
      return;
    }
    if (generation !== this.generation || this.disposed) {
      bitmap.close();
      return;
    }

    if (!this.playing) {
      // A single still frame for a paused seek.
      this.draw(bitmap);
      bitmap.close();
      this.finishStill();
      return;
    }

    const index = tag;
    let at = this.queue.length;
    while (at > 0 && this.queue[at - 1].index > index) at--;
    this.queue.splice(at, 0, { index, bitmap });

    if (this.starting) this.releaseGate();
  }

  // ---- stills (paused display) -------------------------------------------

  private requestStill(seconds: number) {
    if (this.stillBusy) {
      this.stillPending = seconds;
      return;
    }
    this.beginStream(seconds, 1);
    this.stillBusy = true;
  }

  private finishStill() {
    this.stillBusy = false;
    if (this.stillPending !== null) {
      const next = this.stillPending;
      this.stillPending = null;
      this.requestStill(next);
    }
  }

  // ---- render loop --------------------------------------------------------

  private startLoop() {
    if (this.raf) return;
    this.raf = requestAnimationFrame(this.tick);
  }

  private stopLoop() {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
  }

  private tick = () => {
    this.raf = 0;
    if (!this.playing || this.disposed) return;
    this.raf = requestAnimationFrame(this.tick);

    const now = performance.now();

    if (this.starting) {
      if (now - this.gateTime > 10000 && this.queue.length === 0) {
        this.fail("The video stream did not start. Is the file still available?");
      }
      return;
    }

    if (this.useAudio && !this.audioRunning && now - this.gateTime > AUDIO_START_TIMEOUT_MS) {
      this.disableAudio("The audio did not start in time. Playing without sound.");
      return;
    }

    const time = Math.min(this.duration, Math.max(0, this.clock()));
    this.position = time;

    const target = Math.floor((time - this.streamStart) * this.fps + 1e-6);
    let show: QueuedFrame | null = null;
    let consumed = 0;
    while (this.queue.length > 0 && this.queue[0].index <= target) {
      if (show) show.bitmap.close();
      show = this.queue.shift()!;
      consumed++;
    }
    if (show) {
      this.draw(show.bitmap);
      show.bitmap.close();
    }
    if (consumed > 0) {
      this.pendingAcks += consumed;
      this.flushAcks();
    }

    this.events.onTime(time);

    if (time >= this.duration - 0.001) this.finish();
  };

  private finish() {
    this.halt();
    this.position = this.duration;
    this.events.onPlayingChange(false);
    this.events.onTime(this.duration);
  }

  private draw(bitmap: ImageBitmap) {
    if (this.canvas.width !== bitmap.width || this.canvas.height !== bitmap.height) {
      this.canvas.width = bitmap.width;
      this.canvas.height = bitmap.height;
    }
    this.ctx.drawImage(bitmap, 0, 0);
  }

  private flushAcks() {
    const count = this.pendingAcks;
    const pending = this.sessionPromise;
    if (count <= 0 || !pending) return;
    this.pendingAcks = 0;
    void pending.then((id) => {
      if (id >= 0) void invoke("player_ack", { session: id, count }).catch(() => {});
    });
  }

  private fail(message: string) {
    const wasPlaying = this.playing;
    this.halt();
    if (wasPlaying) {
      this.events.onPlayingChange(false);
    }
    this.events.onError(message);
  }

  /** Stops playback and the stream; keeps the media loaded. */
  private halt() {
    this.playing = false;
    this.starting = false;
    this.cancelSeekTimer();
    this.stopLoop();
    this.audio.pause();
    this.endStream();
  }

  private cancelSeekTimer() {
    if (this.seekTimer !== null) window.clearTimeout(this.seekTimer);
    this.seekTimer = null;
  }
}
