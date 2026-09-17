/**
 * useFormat.ts — display formatting for telemetry (PRD UI-2, FR-5.1).
 *
 * Every number the user sees passes through here so units and precision are
 * consistent across the radar, the transfer rows, and history.
 *
 * A note on the guard clauses that open almost every function below: these
 * values originate as Rust integers and floats crossing the IPC boundary as
 * JSON. A rate computed from a zero-length time window arrives as `Infinity`,
 * and an ETA with no progress yet arrives as `null`. Formatting those naively
 * would print "Infinity MB/s" or "NaN", so each function resolves them to a
 * neutral placeholder instead — telemetry that is honest about not knowing.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */

const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/**
 * Bytes as a human-scaled string, e.g. `1.4 GB`.
 */
export function formatBytes(bytes: number, precision = 1): string {
  // Non-finite or negative is not a size; print the em-dash placeholder that
  // the rest of the UI uses for "unknown".
  if (!Number.isFinite(bytes) || bytes < 0) return "—";

  // Below 1 KB, a decimal place is noise: "847 B" not "847.0 B". Returning
  // early also keeps the loop below from running zero times and formatting
  // sub-kilobyte values with the wrong precision.
  if (bytes < 1024) return `${Math.round(bytes)} B`;

  // Repetition: divide down through the unit ladder until the value is under
  // 1024 or we run out of units. The second condition is the important one —
  // without it a petabyte-scale number would index past the end of UNITS and
  // produce "1.2 undefined". Bounded by UNITS.length, so at most 4 iterations.
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(precision)} ${UNITS[unit]}`;
}

/** Rate is shown in MB/s per the mockups, with Mbps as the secondary readout. */
export function formatRate(bytesPerSecond: number): string {
  // A stalled or not-yet-started stream reads as a definite zero rather than a
  // placeholder: the transfer is real, its speed is genuinely 0.
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0 MB/s";

  const mbps = bytesPerSecond / (1024 * 1024);

  // Switch units below 0.1 MB/s. At that scale "0.0 MB/s" would look identical
  // to a stall, when in fact bytes are still moving — on a weak Wi-Fi link that
  // distinction is exactly what the user is watching for.
  if (mbps < 0.1) return `${(bytesPerSecond / 1024).toFixed(0)} KB/s`;

  return `${mbps.toFixed(1)} MB/s`;
}

/**
 * The same rate in bits per second, which is how network links are specified
 * and therefore how §2.3's throughput criteria are written.
 */
export function formatBitrate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0 Mbps";
  // Decimal mega, not binary mebi: link speeds are quoted in powers of ten.
  return `${((bytesPerSecond * 8) / 1_000_000).toFixed(1)} Mbps`;
}

/**
 * Seconds remaining as a compact duration.
 *
 * A descending cascade rather than one general algorithm, because each
 * magnitude wants different wording and precision.
 */
export function formatEta(seconds: number | null): string {
  // `null` is the honest answer before the rolling average has a window to
  // work with (FR-5.2); `Infinity` arrives when the rate is zero.
  if (seconds === null || !Number.isFinite(seconds)) return "—";

  // Sub-second: counting down "0s" repeatedly looks frozen, so state the bound.
  if (seconds < 1) return "<1s";

  // Under a minute, seconds alone are precise enough to be useful.
  if (seconds < 60) return `${Math.round(seconds)}s`;

  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);

  // Under an hour, show both parts — "3m 20s" is more legible than "200s".
  if (minutes < 60) return `${minutes}m ${rest}s`;

  // An hour or more: seconds are below the noise floor of the estimate itself,
  // so they are dropped entirely rather than implying false precision.
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

/**
 * Progress as a percentage, clamped to 0–100.
 *
 * Returns a number, not a string, because callers feed it to both a label and
 * a CSS width.
 */
export function formatPercent(done: number, total: number): number {
  // A zero total would divide to Infinity or NaN. It legitimately occurs for a
  // batch of empty files (FR-2.11), where "done" is the only sensible reading
  // once any bytes — or the entries themselves — have been accounted for.
  if (total <= 0) return done > 0 ? 100 : 0;

  // Clamped because UI-3 forbids a progress bar that overshoots or rewinds, and
  // a receiver's byte count can momentarily exceed the declared size when a
  // chunk boundary and a progress tick interleave.
  return Math.min(100, Math.max(0, (done / total) * 100));
}

/**
 * A timestamp as "just now" / "5m ago" / a date.
 *
 * Another descending cascade: recency matters most for the newest entries, and
 * an exact clock time is useless for something that happened days ago.
 */
export function formatRelativeTime(timestampMs: number): string {
  const delta = Date.now() - timestampMs;

  // Within the last minute — finer granularity than this would make the history
  // list appear to change while the user is reading it.
  if (delta < 60_000) return "just now";

  // Within the hour, then within the day.
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)}h ago`;

  // Older than a day: an absolute date is more meaningful than "37h ago".
  // Locale-aware, and with no year — history is capped at 200 entries
  // (FR-5.4), so it rarely spans one.
  return new Date(timestampMs).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/** Keeps long paths readable in a fixed-width row without a tooltip. */
export function truncateMiddle(text: string, max = 42): string {
  // Nothing to do, and returning early avoids the slicing below producing an
  // ellipsis in a string that already fits.
  if (text.length <= max) return text;

  // Split the budget across both ends so the filename at the tail stays
  // visible — the middle of a path is the least informative part. `ceil` on the
  // head and `floor` on the tail spends the odd character where it reads
  // better, and the `- 1` reserves room for the ellipsis itself.
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return `${text.slice(0, head)}…${text.slice(-tail)}`;
}

/** The last segment of a path, accepting either separator. */
export function fileName(path: string): string {
  // Splits on both `/` and `\` because paths cross platforms here: a manifest
  // built on Windows is displayed on Android and vice versa. `?? path` covers
  // an empty string, where `pop()` yields undefined.
  return path.split(/[\\/]/).pop() ?? path;
}
