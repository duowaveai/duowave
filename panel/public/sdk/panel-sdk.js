// «@duowave/panel-sdk» для плагинов пульта: definePlugin, useApi, useSettings и компоненты «Стекла» (src/sdk/index.ts).
// Типы — docs/plugins/panel-sdk.d.ts.
const sdk = globalThis.__duowave.sdk;
export const { definePlugin, useApi, useSettings, usePanelState, can, Card, Action, Switch, Sheet, Note, Pill } = sdk;
export default sdk;
