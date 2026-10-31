/// <reference types="vite/client" />

// Module shim so `bun check` (which does not parse .vue SFCs) can resolve
// component imports from the TypeScript layer. The SFC internals stay
// checked by the vite build; this file only describes the import shape.
declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<
    Record<string, never>,
    Record<string, never>,
    unknown
  >;
  export default component;
}
