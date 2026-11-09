// Плагинам пульта — тот же Preact, что у пульта: одна копия на странице (docs/plans/plugins.md, этап 5). import map в
// index.html ведёт сюда «preact»; пульт кладёт свой Preact в globalThis.__duowave до загрузки плагинов (src/sdk/index.ts).
const preact = globalThis.__duowave.preact;
export const { h, createElement, Fragment, render, hydrate, Component, createContext, createRef, cloneElement, toChildArray, isValidElement, options } = preact;
export default preact;
