/**
 * vite-env.d.ts — ambient type declarations for the Vite build.
 *
 * Gives TypeScript the shape of Vite's client globals and teaches it that a
 * `*.vue` import resolves to a component. Without the module declaration,
 * every SFC import in the project would be a type error.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */
/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<Record<string, unknown>, Record<string, unknown>, unknown>;
  export default component;
}
