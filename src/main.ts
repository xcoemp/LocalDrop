/**
 * main.ts — frontend entry point.
 *
 * Mounts the Vue application and installs Pinia. Deliberately minimal: the
 * app owns no routing (view switching is local state in `App.vue`) and no
 * HTTP client, because LocalDrop makes no network requests from the webview.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */
import { createApp } from "vue";
import { createPinia } from "pinia";

import App from "@/App.vue";
import "@/style.css";

// No control flow here by design — a failure to mount is unrecoverable and is
// better surfaced as an uncaught error in devtools than swallowed by a guard.
createApp(App).use(createPinia()).mount("#app");
