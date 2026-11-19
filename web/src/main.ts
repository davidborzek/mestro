import { mount } from "svelte";
import App from "./App.svelte";
import { initRouter } from "./lib/router.svelte";
import "./theme.css";

// the dev mock (?mock) feeds a fake kiosk so screens can be
// developed and screenshotted without hardware. The dynamic
// import sits behind import.meta.env.DEV, so production builds
// drop it (and the whole mock module) entirely.
async function bootstrap() {
  if (import.meta.env.DEV && new URLSearchParams(location.search).has("mock")) {
    const { installMock, installMeasureProbe, fillPinInput } = await import("./lib/mock");
    installMock();
    installMeasureProbe();
    fillPinInput();
  }
  // route first so the app renders the URL's screen directly
  initRouter();
  return mount(App, { target: document.getElementById("app")! });
}

const app = await bootstrap();

export default app;
