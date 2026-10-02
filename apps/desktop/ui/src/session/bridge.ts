// The Tauri side of the engine contract: typed wrappers over the commands and
// events in src-tauri/src/lib.rs. See BRIDGE.md.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AudioDevice } from './types';

/** True inside the desktop app; false in a browser or the preview. */
export function inDesktopApp(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export interface EngineInfo {
  version: string;
  /** host:port of the room server this build uses, or null if none is set up. */
  roomServer: string | null;
  /** Live audio between the booths (waits for the engine's device mode). */
  liveAudio: boolean;
}

export interface Paired {
  /** "K7QX-M2PD" */
  code: string;
  peerName: string;
  isHost: boolean;
  relay: boolean;
}

export type RoomEvent = ({ kind: 'paired' } & Paired) | { kind: 'error'; message: string };

export interface NetworkResult {
  pingsSent: number;
  pongsReceived: number;
  lossPct: number;
  rttMs: number;
  rttMinMs: number;
  jitterMs: number;
  clockOffsetMs: number | null;
  reached: boolean;
}

export interface LinkStatus {
  state: 'connected' | 'reconnecting' | 'left';
  rttMs: number;
  jitterMs: number;
  lossPct: number;
  relay: boolean;
}

/** The engine's live session, as `obsidian_live::LiveStatus` serializes it. */
export interface LiveStatus {
  /** "waiting for partner", "measuring the network", "live", "partner not reachable", "partner left" */
  phase: string;
  partner_name: string | null;
  on_air: boolean;
  partner_on_air: boolean;
  booth_delay_ms: number | null;
  rtt_ms: number | null;
  margin_ms: number | null;
  recovered_10s: number;
  concealed_10s: number;
  /** Linear peak since the last status. */
  local_peak: number;
  partner_peak: number;
  send_kbps: number;
}

/** dBFS; null is silence (JSON has no -Infinity). */
export interface WireLevel {
  left: number | null;
  right: number | null;
}

export const bridge = {
  engineInfo: () => invoke<EngineInfo>('engine_info'),
  listDevices: () => invoke<{ inputs: AudioDevice[]; outputs: AudioDevice[] }>('list_devices'),
  startInputMeter: (id: string) => invoke<void>('start_input_meter', { id }),
  stopInputMeter: () => invoke<void>('stop_input_meter'),
  /** Resolves with the code at once; the other DJ arriving comes as a room event. */
  createRoom: (name: string) => invoke<string>('create_room', { name }),
  joinRoom: (code: string, name: string) => invoke<Paired>('join_room', { code, name }),
  leaveRoom: () => invoke<void>('leave_room'),
  sendControl: (msg: unknown) => invoke<void>('send_control', { json: JSON.stringify(msg) }),
  runNetworkTest: (seconds: number) => invoke<NetworkResult>('run_network_test', { seconds }),
  selectOutput: (id: string) => invoke<void>('select_output', { id }),
  /** The countdown is over: the engine takes the connection, your input and headphones. */
  startLive: (startOnAir: boolean) => invoke<void>('start_live', { startOnAir }),
  liveTakeOver: () => invoke<void>('live_take_over'),
  liveSetFader: (value: number) => invoke<void>('live_set_fader', { value }),
  liveSetPartnerVolume: (value: number) => invoke<void>('live_set_partner_volume', { value }),
  /** Ends the set; resolves with the folder its recordings went to. */
  stopLive: () => invoke<string | null>('stop_live'),

  onRoomEvent: (f: (e: RoomEvent) => void) => listen<RoomEvent>('room-event', (e) => f(e.payload)),
  onPeerControl: (f: (json: string) => void) => listen<string>('peer-control', (e) => f(e.payload)),
  onLinkStatus: (f: (s: LinkStatus) => void) => listen<LinkStatus>('link-status', (e) => f(e.payload)),
  onLocalLevel: (f: (l: WireLevel) => void) => listen<WireLevel>('local-level', (e) => f(e.payload)),
  onRemoteLevel: (f: (l: WireLevel) => void) => listen<WireLevel>('remote-level', (e) => f(e.payload)),
  onLiveStatus: (f: (s: LiveStatus) => void) => listen<LiveStatus>('live-status', (e) => f(e.payload)),
};

export type Unlisten = UnlistenFn;
export type Bridge = typeof bridge;
