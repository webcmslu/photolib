import { mount } from "svelte";
import "../app.css";
import "./tv.css";
import TvApp from "./TvApp.svelte";

mount(TvApp, { target: document.getElementById("app")! });
