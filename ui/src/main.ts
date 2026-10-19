import { mount } from "svelte";
import "./tokens.css";
import "./app.css";
import App from "./App.svelte";
import { strings } from "./strings";

document.title = strings.appName;

const target = document.getElementById("app");
if (target) {
  mount(App, { target });
}
