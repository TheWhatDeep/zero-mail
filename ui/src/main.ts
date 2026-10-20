import { mount } from "svelte";
import "./tokens.css";
import "./app.css";
import App from "./App.svelte";
import Compose from "./components/Compose.svelte";
import type { DraftView } from "./lib/ipc";
import { strings } from "./strings";

declare global {
  interface Window {
    /** Set by the app's start-up script in a compose window (SPEC_GAPS G63). */
    __ZEROMAIL_DRAFT__?: DraftView;
    /** Saves the draft now; the app calls it when a compose window closes. */
    __ZEROMAIL_FLUSH__?: () => void;
  }
}

document.title = strings.appName;

const target = document.getElementById("app");
const draft = window.__ZEROMAIL_DRAFT__;
if (target && location.hash.startsWith("#compose/") && draft) {
  document.title = draft.content.subject || strings.compose;
  mount(Compose, { target, props: { view: draft } });
} else if (target) {
  mount(App, { target });
}
