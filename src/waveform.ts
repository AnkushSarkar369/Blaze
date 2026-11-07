// Audacity-style waveform rendering.
//
// The backend sends a per-millisecond summary (min / max / RMS per block, see
// src-tauri/src/peaks.rs). Drawing walks the pixel columns that are actually on
// screen, so cost depends on the viewport width, never on the length of the file.

const HEADER_BYTES = 16;
const MAGIC = 0x42575631;

export interface WaveformData {
  channels: number;
  blocksPerSecond: number;
  blocks: number;
  min: Int16Array[];
  max: Int16Array[];
  rms: Uint16Array[];
}

export interface WaveformColors {
  peak: string;
  rms: string;
  center: string;
  clip: string;
  divider: string;
}

export interface DrawOptions {
  scrollLeft: number;
  pixelsPerSecond: number;
  /** Override the measured size (used when the canvas has no layout, e.g. tests). */
  cssWidth?: number;
  cssHeight?: number;
  dpr?: number;
  colors?: Partial<WaveformColors>;
}

const DEFAULT_COLORS: WaveformColors = {
  peak: "#4e8f7c",
  rms: "#a9e0cf",
  center: "#2f453f",
  clip: "#ff5a4d",
  divider: "#2a2d31",
};

export function parseWaveform(buffer: ArrayBuffer): WaveformData {
  if (buffer.byteLength < HEADER_BYTES) throw new Error("Waveform data is truncated.");
  const view = new DataView(buffer);
  if (view.getUint32(0, true) !== MAGIC) throw new Error("Unrecognised waveform data.");

  const channels = view.getUint32(4, true);
  const blocksPerSecond = view.getUint32(8, true);
  const blocks = view.getUint32(12, true);
  if (channels < 1 || channels > 2 || blocksPerSecond < 1 || blocks < 1) {
    throw new Error("Waveform data is empty or invalid.");
  }
  if (buffer.byteLength < HEADER_BYTES + channels * blocks * 6) {
    throw new Error("Waveform data is truncated.");
  }

  const min: Int16Array[] = [];
  const max: Int16Array[] = [];
  const rms: Uint16Array[] = [];
  let offset = HEADER_BYTES;
  for (let channel = 0; channel < channels; channel++) {
    min.push(new Int16Array(buffer, offset, blocks));
    offset += blocks * 2;
    max.push(new Int16Array(buffer, offset, blocks));
    offset += blocks * 2;
    rms.push(new Uint16Array(buffer, offset, blocks));
    offset += blocks * 2;
  }
  return { channels, blocksPerSecond, blocks, min, max, rms };
}

export function drawWaveform(canvas: HTMLCanvasElement, data: WaveformData, options: DrawOptions) {
  const dpr = options.dpr ?? (globalThis.devicePixelRatio || 1);
  const cssWidth = options.cssWidth ?? canvas.clientWidth;
  const cssHeight = options.cssHeight ?? canvas.clientHeight;
  const width = Math.round(cssWidth * dpr);
  const height = Math.round(cssHeight * dpr);
  if (width <= 0 || height <= 0) return;
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.clearRect(0, 0, width, height);

  const colors = { ...DEFAULT_COLORS, ...options.colors };
  const pixelsPerSecond = options.pixelsPerSecond;
  const blocksPerSecond = data.blocksPerSecond;
  const gap = data.channels > 1 ? Math.max(1, Math.round(dpr)) : 0;
  const laneHeight = Math.floor((height - gap * (data.channels - 1)) / data.channels);
  const padding = Math.round(4 * dpr);
  const unit = Math.max(1, Math.round(dpr));

  for (let lane = 0; lane < data.channels; lane++) {
    const top = lane * (laneHeight + gap);
    const middle = top + laneHeight / 2;
    const half = Math.max(1, laneHeight / 2 - padding);
    const mins = data.min[lane];
    const maxs = data.max[lane];
    const rmss = data.rms[lane];

    if (lane > 0) {
      ctx.fillStyle = colors.divider;
      ctx.fillRect(0, top - gap, width, gap);
    }

    // Zero line, visible even through silence.
    ctx.fillStyle = colors.center;
    ctx.fillRect(0, Math.round(middle), width, unit);

    for (let x = 0; x < width; x++) {
      const start = ((options.scrollLeft + x / dpr) / pixelsPerSecond) * blocksPerSecond;
      const end = start + blocksPerSecond / (dpr * pixelsPerSecond);
      if (end <= 0) continue;
      if (start >= data.blocks) break;

      let low: number;
      let high: number;
      let level: number;

      if (end - start <= 1) {
        // Zoomed in past one block per pixel: interpolate between block centres
        // so the envelope stays smooth instead of stepping.
        const position = Math.min(data.blocks - 1, Math.max(0, (start + end) / 2 - 0.5));
        const first = Math.floor(position);
        const second = Math.min(first + 1, data.blocks - 1);
        const mix = position - first;
        low = mins[first] * (1 - mix) + mins[second] * mix;
        high = maxs[first] * (1 - mix) + maxs[second] * mix;
        level = rmss[first] * (1 - mix) + rmss[second] * mix;
      } else {
        const from = Math.max(0, Math.floor(start));
        const to = Math.min(data.blocks, Math.max(from + 1, Math.ceil(end)));
        low = 32767;
        high = -32768;
        let squares = 0;
        for (let block = from; block < to; block++) {
          if (mins[block] < low) low = mins[block];
          if (maxs[block] > high) high = maxs[block];
          squares += rmss[block] * rmss[block];
        }
        level = Math.sqrt(squares / (to - from));
      }

      const peakTop = middle - (high / 32768) * half;
      const peakBottom = middle - (low / 32768) * half;
      ctx.fillStyle = colors.peak;
      ctx.fillRect(x, peakTop, 1, Math.max(1, peakBottom - peakTop));

      // RMS body, clamped inside the peak envelope like Audacity does.
      const bodyHigh = Math.min(high, level);
      const bodyLow = Math.max(low, -level);
      if (bodyHigh > bodyLow) {
        const bodyTop = middle - (bodyHigh / 32768) * half;
        const bodyBottom = middle - (bodyLow / 32768) * half;
        ctx.fillStyle = colors.rms;
        ctx.fillRect(x, bodyTop, 1, Math.max(1, bodyBottom - bodyTop));
      }

      // Clipped samples get a red cap.
      if (high >= 32767 || low <= -32768) {
        ctx.fillStyle = colors.clip;
        if (high >= 32767) ctx.fillRect(x, peakTop, 1, unit);
        if (low <= -32768) ctx.fillRect(x, peakBottom - unit, 1, unit);
      }
    }
  }
}
