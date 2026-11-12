// «@duowave/panel-sdk» для плагинов пульта: definePlugin, useApi, useSettings, картинки своего плагина (useFile), живые
// данные колонки (useLive, useStation), значки и компоненты пульта, MIDI-клавиатура (useMidi, useSynth, PianoKeys,
// readMidiFile, melodyOf, noteName — src/sdk/index.tsx).
// Типы — docs/plugins/panel-sdk.d.ts.
const sdk = globalThis.__duowave.sdk;
export const { definePlugin, useApi, useSettings, useFile, usePanelState, useLive, useStation, icons, can, Card, Action, Switch, Sheet, Note, Pill, ask } = sdk;
export const { useMidi, useSynth, PianoKeys, readMidiFile, melodyOf, noteName } = sdk;
export default sdk;
