// Domain types for the Live Session screen.
//
// Three kinds of state are kept apart on purpose:
//   - SessionState: who owns the booth, readiness, handoffs. Slow, event-driven,
//     and the source of the handoff log used later for the master rebuild.
//   - LinkState: connection and recording health, plus Diagnostics numbers.
//     Comes from the audio/network engine.
//   - StereoLevel: the remote meter. High rate, so it has its own subscription.
// UI-only state (drawer open, confirm popover, hold progress) lives in components.

export type DjId = string;

/** The four roles a DJ can show on the Live Session screen. */
export type BoothRole = 'onAir' | 'cueing' | 'ready' | 'handoff';

export interface TrackInfo {
  title: string;
  artist: string;
  /** Seconds left in the track, if the engine knows it. */
  remainingSec: number | null;
}

export interface Dj {
  id: DjId;
  name: string;
  city: string;
  isLocal: boolean;
  /** Profile photo shown while this DJ owns the mix. */
  photoUrl?: string;
  track: TrackInfo | null;
}

export interface HandoffState {
  from: DjId;
  to: DjId;
  startedAtMs: number;
  durationMs: number;
}

/** Mirrors the session_events table proposed in the feasibility report. */
export type SessionEventType =
  | 'start'
  | 'ready'
  | 'ready_cancelled'
  | 'take_over'
  | 'handoff_complete'
  | 'emergency_take_over'
  | 'remote_reconnecting'
  | 'remote_reconnected'
  | 'end';

export interface SessionEvent {
  type: SessionEventType;
  atMs: number;
  djId?: DjId;
}

export interface SessionState {
  status: 'live' | 'ended';
  startedAtMs: number;
  endedAtMs: number | null;
  localId: DjId;
  remoteId: DjId;
  djs: Record<DjId, Dj>;
  /** The DJ who currently owns the mix (ON AIR). */
  ownerId: DjId;
  readyIds: DjId[];
  handoff: HandoffState | null;
  events: SessionEvent[];
}

export type RemoteConnection = 'connected' | 'reconnecting' | 'left';
export type BoothSync = 'stable' | 'adjusting' | 'unstable' | 'lost';
export type NetworkQuality = 'excellent' | 'good' | 'fair' | 'poor' | 'recovering';
export type RecordingState = 'on' | 'off';

/** Technical numbers. Only ever shown inside the Diagnostics drawer. */
export interface Diagnostics {
  roundTripMs: number;
  oneWayMs: number;
  jitterMs: number;
  packetLossPct: number;
  bufferMs: number;
  clockDriftPpm: number;
  path: 'direct' | 'relay';
  codec: string;
}

export interface LinkState {
  remote: RemoteConnection;
  boothSync: BoothSync;
  network: NetworkQuality;
  recording: RecordingState;
  diagnostics: Diagnostics;
}

/** dBFS per channel; -Infinity is silence. */
export interface StereoLevel {
  left: number;
  right: number;
}

// ---------- Before the session: room and booth check ----------------------
//
// RoomState covers Home (create or join) and Booth Check (devices, checks,
// ready). It is separate from SessionState: a session only exists once both
// DJs are ready and the room enters 'live'.

export type RoomPhase = 'home' | 'booth' | 'live';

/** The other DJ, as seen from this booth before the session starts. */
export type RemotePresence = 'waiting' | 'joined' | 'ready';

export interface AudioDevice {
  id: string;
  /** Interface or app name, e.g. "Behringer UMC204HD". */
  label: string;
  /** Channels or route, e.g. "Input 1-2". */
  detail: string;
}

export type CheckStepId = 'network' | 'roundTrip' | 'send' | 'receive' | 'sync';
export type CheckStepStatus = 'pending' | 'running' | 'ok' | 'attention';

export interface CheckStep {
  id: CheckStepId;
  label: string;
  status: CheckStepStatus;
  /** Plain-language result once the step has run. */
  result: string | null;
}

export interface BoothCheck {
  status: 'idle' | 'running' | 'passed' | 'attention';
  steps: CheckStep[];
}

export interface RoomState {
  phase: RoomPhase;
  /** Invite code, "K7QX-M2PD". Null on Home. */
  code: string | null;
  isHost: boolean;
  /** True while a join is being looked up. */
  joining: boolean;
  joinError: string | null;
  /** True while a new room is being opened. */
  creating: boolean;
  createError: string | null;
  local: { name: string; city: string; photoUrl?: string };
  remote: { name: string; city: string; photoUrl?: string } | null;
  remotePresence: RemotePresence;
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  inputId: string | null;
  outputId: string | null;
  check: BoothCheck;
  localReady: boolean;
  /** Set when both DJs are ready: the session starts at this time. */
  startsAtMs: number | null;
}
