// Console bootstrap. Kept out of index.html so the gateway's /console/ CSP
// (script-src 'self' 'wasm-unsafe-eval', no 'unsafe-inline') allows it.
import init, { mgr_start } from "./pkg/weftos_cog_manager.js";

await init();
await mgr_start("mgr_canvas");
document.getElementById("loading")?.remove();
