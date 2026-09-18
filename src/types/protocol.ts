/**
 * protocol.ts — TypeScript mirrors of the Rust protocol and state types
 * (PRD §6).
 *
 * These must stay in lockstep with src-tauri/src/{protocol,state,registry,
 * settings,history}.rs — the Rust side serializes with `rename_all = "camelCase"`.
 *
 * Types only: no runtime code, and therefore no control flow. The one exported
 * value is the `EVENTS` map at the foot of the file. Keeping the mirrors here
 * and nowhere else means a change to a Rust struct produces a type error at
 * every consumer rather than a silently-undefined field at runtime.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */

/**
 * Mirrors `PeerOs` in protocol.rs. `unknown` is reachable: a peer may report an
 * OS this build does not know (see the `default` arm in OsIcon.vue).
 */
export type PeerOs = "windows" | "android" | "macos" | "linux" | "unknown";
/** §6.2 — `busy` renders a peer card non-droppable. */
export type PeerLiveState = "ready" | "busy";

export interface Peer {
  deviceId: string;
  alias: string;
  os: PeerOs;
  ip: string;
  tcpPort: number;
  clientVersion: string;
  state: PeerLiveState;
  lastSeenMs: number;
}

export interface IncompatiblePeer {
  deviceId: string;
  alias: string;
  ip: string;
  clientVersion: string;
  protocolVersion: number;
}

export type Direction = "incoming" | "outgoing";

/**
 * §6.4's state machine, flattened into a union:
 * `Queued → Offered → (Accepted | Rejected) → Transferring → (Completed |
 * Cancelled | Failed)`.
 *
 * `accepted` has no variant here because it is not an observable resting state
 * — acceptance moves straight to `transferring`. The four terminal phases are
 * the ones `useTransferStore`'s `TERMINAL` set matches on.
 */
export type TransferPhase =
  | "queued"
  | "offered"
  | "transferring"
  | "completed"
  | "cancelled"
  | "failed"
  | "rejected";

/** §6.4 — the snapshot both ends render identically. */
export interface TransferSnapshot {
  id: string;
  direction: Direction;
  peerAlias: string;
  peerDeviceId: string;
  phase: TransferPhase;
  label: string;
  currentFile: string;
  index: number;
  count: number;
  bytesDone: number;
  totalBytes: number;
  rateBps: number;
  /** Null until the rolling window has data — see `formatEta` (FR-5.2). */
  etaSeconds: number | null;
  isText: boolean;
}

export interface OfferPayload {
  id: string;
  peerAlias: string;
  peerDeviceId: string;
  itemCount: number;
  totalBytes: number;
  isText: boolean;
  preview: string[];
}

export interface TransferErrorPayload {
  id: string;
  direction: Direction;
  peerAlias: string;
  code: string;
  message: string;
  detail: string | null;
}

export type Outcome = "completed" | "cancelled" | "failed" | "rejected";

export interface HistoryEntry {
  id: string;
  direction: Direction;
  peerAlias: string;
  peerDeviceId: string;
  label: string;
  itemCount: number;
  totalBytes: number;
  outcome: Outcome;
  finishedAt: number;
  /** Null for an outgoing transfer or a snippet — there is nothing to reveal. */
  path: string | null;
  /** Null unless this was a snippet; carries the body for re-copying (FR-3.3). */
  text: string | null;
  /** Both null unless `outcome` is `failed` (FR-5.6). */
  errorCode: string | null;
  errorMessage: string | null;
  /** FR-2.3 — relative paths of symlinks and unreadable entries. */
  skipped: string[];
}

export type CollisionPolicy = "rename" | "overwrite" | "skip";

export interface Settings {
  deviceAlias: string;
  downloadDirectory: string;
  autoAccept: boolean;
  autoCopySnippets: boolean;
  completionSound: boolean;
  /** A short tone when a transfer starts moving. Opt-in; see `settings.rs`. */
  transferStartSound: boolean;
  launchMinimized: boolean;
  organizeBySender: boolean;
  organizeByFileType: boolean;
  collisionPolicy: CollisionPolicy;
  /** AND-2 — keep a foreground service alive so backgrounded transfers survive. */
  backgroundService: boolean;
  discoveryPort: number;
  transferPort: number;
}

/** FR-6.4's read-only diagnostics. Note: no gateway — see §15.1. */
export interface NetworkSnapshot {
  /** FR-1.7 — false drives the "No LAN connection" empty state. */
  online: boolean;
  /** All four are null when there is no usable interface. */
  interface: string | null;
  localIp: string | null;
  netmask: string | null;
  /** §6.2 — directed broadcast addresses, one per usable interface. */
  broadcastTargets: string[];
  /** The port actually bound, which may differ from 57321/57322 after fallback. */
  discoveryPort: number;
  transferPort: number;
}

export interface DeviceInfo {
  deviceId: string;
  alias: string;
  os: PeerOs;
  clientVersion: string;
  protocolVersion: number;
  /** §8 — false in v1.0. The UI reads this instead of asserting encryption. */
  encrypted: boolean;
}

/** State of the Android background listener (AND-2). */
export interface BackgroundStatus {
  enabled: boolean;
  /** False means Doze can still freeze long transfers. */
  batteryUnrestricted: boolean;
}

export interface SendTextResult {
  id: string;
  /** FR-3.4 — true when the snippet exceeded 1 MB and became a .txt file. */
  converted: boolean;
}

/**
 * Rust → frontend event names (§6.1).
 *
 * Centralised and `as const` so a listener cannot subscribe to a misspelled
 * channel. A typo in a string literal at the `listen` call site would compile,
 * run, and simply never fire — the hardest class of bug to notice in an
 * event-driven UI.
 */
export const EVENTS = {
  peerDiscovered: "peer://discovered",
  peerUpdated: "peer://updated",
  peerLost: "peer://lost",
  peerIncompatible: "peer://incompatible",
  transferOffer: "transfer://offer",
  transferProgress: "transfer://progress",
  transferComplete: "transfer://complete",
  transferError: "transfer://error",
  networkChanged: "network://changed",
  settingsChanged: "settings://changed",
} as const;
