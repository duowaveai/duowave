// «preact/jsx-runtime» для плагинов пульта, собранных с JSX (esbuild --jsx=automatic --jsx-import-source=preact).
const runtime = globalThis.__duowave.jsxRuntime;
export const { jsx, jsxs, jsxDEV, Fragment } = runtime;
export default runtime;
