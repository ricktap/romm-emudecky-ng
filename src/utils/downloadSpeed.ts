/**
 * Transfer speed and time remaining, measured in the UI from the byte counts the progress events already carry
 * (no extra backend traffic). Speed is a smoothed bytes/second per download.
 */

interface Sample {
  bytes: number;
  at: number;
  speed: number;
}

const samples = new Map<string, Sample>();
const SMOOTHING = 0.3;
const MIN_WINDOW_MS = 800;

/** Feed the latest byte count of a download; returns its smoothed speed (0 until two samples are far enough apart). */
export function recordBytes(key: string, bytes: number, nowMs: number): number {
  const prev = samples.get(key);
  if (!prev || bytes < prev.bytes) {
    samples.set(key, { bytes, at: nowMs, speed: 0 });
    return 0;
  }
  const elapsed = nowMs - prev.at;
  if (elapsed < MIN_WINDOW_MS) return prev.speed;
  const instant = ((bytes - prev.bytes) * 1000) / elapsed;
  const speed = prev.speed === 0 ? instant : prev.speed * (1 - SMOOTHING) + instant * SMOOTHING;
  samples.set(key, { bytes, at: nowMs, speed });
  return speed;
}

export function forgetSpeed(key: string): void {
  samples.delete(key);
}

export function formatSpeed(bytesPerSecond: number): string {
  if (bytesPerSecond < 1) return "";
  if (bytesPerSecond < 1024 * 1024) return `${Math.round(bytesPerSecond / 1024)} KB/s`;
  return `${(bytesPerSecond / (1024 * 1024)).toFixed(1)} MB/s`;
}

/** "5 min left" style estimate; empty when speed or total is unknown. */
export function formatEta(remainingBytes: number, bytesPerSecond: number): string {
  if (bytesPerSecond < 1 || remainingBytes <= 0) return "";
  const seconds = Math.ceil(remainingBytes / bytesPerSecond);
  if (seconds < 60) return `${seconds} s left`;
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `${minutes} min left`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min left`;
}
