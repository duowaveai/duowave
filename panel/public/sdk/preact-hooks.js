// «preact/hooks» для плагинов пульта — хуки самого пульта (src/sdk/index.ts).
const hooks = globalThis.__duowave.hooks;
export const { useState, useReducer, useEffect, useLayoutEffect, useRef, useImperativeHandle, useMemo, useCallback, useContext, useDebugValue, useErrorBoundary, useId } = hooks;
export default hooks;
