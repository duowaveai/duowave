// The gateway's features (gateway/src/core/availability.rs), written by its tests: do not edit by hand.
// DUOWAVE_WRITE_PANEL_IDS=1 cargo test -p duowave availability writes it again after a change.

export const FEATURE_IDS = [
    'mode.relay',
    'mode.gateway',
    'mode.own',
    'asr.own',
    'voice.own',
    'voice.cloned',
    'brain.cloud',
    'brain.local',
    'skills',
    'mcp.http',
    'mcp.programs',
    'connectors',
    'messages',
    'calls',
    'stations.connect',
    'voice.lab',
    'characters',
    'characters.voice',
    'plugins',
    'plugins.apps',
    'media.transcode',
    'offline',
] as const;

export type FeatureId = (typeof FEATURE_IDS)[number];
