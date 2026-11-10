// «@duowave/panel-sdk» для плагинов пульта: definePlugin, useApi, useSettings, живые данные колонки (useLive,
// useStation), значки и компоненты пульта (src/sdk/index.tsx).
// Типы — docs/plugins/panel-sdk.d.ts.
const sdk = globalThis.__duowave.sdk;
export const { definePlugin, useApi, useSettings, usePanelState, useLive, useStation, icons, can, Card, Action, Switch, Sheet, Note, Pill, ask } = sdk;
export default sdk;
