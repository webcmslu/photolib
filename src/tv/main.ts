import { mount } from "svelte";
import "../app.css";
import "./tv.css";
import TvApp from "./TvApp.svelte";

// The app is laid out for 1920 x 1080. TVs whose web view is smaller (most
// Android TVs give 960 x 540) see it scaled to fit, as a TV picture.
function fit() {
  const scale = Math.min(window.innerWidth / 1920, window.innerHeight / 1080);
  (document.body.style as CSSStyleDeclaration & { zoom: string }).zoom = String(scale);
}
fit();
window.addEventListener("resize", fit);

mount(TvApp, { target: document.getElementById("app")! });
