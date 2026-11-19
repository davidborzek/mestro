// the router core's browser binding: the reactive `route`, the
// real history and the scroll containers. Pure logic + tests live
// in router.ts.

import { tick } from "svelte";
import {
  createRouter,
  HOME_ROUTE,
  type HistoryShim,
  type Route,
  type ScrollMap,
  type ScrollSink,
} from "./router";

export const route = $state<Route>({ ...HOME_ROUTE });

// scroll containers beyond the window, registered with a stable
// id via `use:scrollable` so their positions survive navigation
const containers = new Map<string, HTMLElement>();

export function scrollable(element: HTMLElement, id: string) {
  containers.set(id, element);
  return {
    destroy() {
      if (containers.get(id) === element) containers.delete(id);
    },
  };
}

const historyShim: HistoryShim = {
  get state() {
    return window.history.state;
  },
  get url() {
    return location.pathname;
  },
  pushState(state, url) {
    window.history.pushState(state, "", url);
  },
  replaceState(state, url) {
    window.history.replaceState(state, "", url ?? undefined);
  },
  back() {
    window.history.back();
  },
};

const scrollSink: ScrollSink = {
  capture() {
    const map: ScrollMap = { window: window.scrollY };
    for (const [id, element] of containers) map[id] = element.scrollTop;
    return map;
  },
  fits(map) {
    return Object.entries(map).every(([id, offset]) => {
      if (id === "window") {
        return offset === 0 || window.innerHeight + offset <= document.documentElement.scrollHeight;
      }
      const element = containers.get(id);
      // a pane that is not mounted yet counts as not fitting —
      // the retry loop waits for it to render
      return (
        element !== undefined &&
        (offset === 0 || element.clientHeight + offset <= element.scrollHeight)
      );
    });
  },
  async settled() {
    await tick();
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  },
  apply(map) {
    for (const [id, element] of containers) element.scrollTop = map[id] ?? 0;
    window.scrollTo(0, map.window ?? 0);
  },
};

const core = createRouter({
  history: historyShim,
  scroll: scrollSink,
  onRoute: (next) => Object.assign(route, next),
});

export const navigate = core.navigate;
export const goBack = core.goBack;

// own the scroll restoration and index the current entry; call
// once before mounting the app
export function initRouter(): void {
  history.scrollRestoration = "manual";
  window.addEventListener("popstate", () => core.onPopState());
  core.init();
}
