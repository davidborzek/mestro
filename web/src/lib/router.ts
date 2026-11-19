// the router core: path matching plus history/scroll bookkeeping,
// DOM-free so tests can drive it with fakes. The browser binding
// (the reactive `route`, real history, scroll containers) lives in
// router.svelte.ts.

export type Page = "remote" | "settings" | "setup";

export interface Route {
  page: Page;
  path: string;
  // the open settings section and, one level deeper, the VU skin
  // whose options are open — both straight from the URL
  section: string | null;
  skin: string | null;
}

export const HOME_ROUTE: Route = { page: "remote", path: "/", section: null, skin: null };

// backend endpoints the SPA must never claim as its own routes
export const RESERVED_PATHS = [
  "/ws",
  "/pair",
  "/pair/status",
  "/cover",
  "/ota",
  "/vu-preview",
  "/clock-preview",
];

// legacy paths fold onto their new home (the System hub became
// the settings Info section) so old links keep working
const REDIRECTS: Record<string, string> = { "/system": "/settings/info" };

function normalize(pathname: string): string {
  return pathname.replace(/\/+$/, "") || "/";
}

// /settings, /settings/:section and — only for the VU skins —
// /settings/:section/:skin
const SETTINGS_PATH = /^\/settings(?:\/([^/]+))?(?:\/([^/]+))?$/;

// the first-boot wifi setup: the captive portal sends phones
// here straight from the hotspot
const SETUP_WIFI_PATH = /^\/setup\/wifi$/;

export function matchRoute(pathname: string): Route | null {
  const path = normalize(pathname);
  if (path === "/") return { ...HOME_ROUTE };
  const target = REDIRECTS[path];
  if (target) return matchRoute(target);
  if (SETUP_WIFI_PATH.test(path)) return { page: "setup", path, section: null, skin: null };
  const settings = SETTINGS_PATH.exec(path);
  if (!settings) return null;
  return {
    page: "settings",
    path,
    section: settings[1] ?? null,
    skin: settings[2] ?? null,
  };
}

// scroll offsets by container id: "window" plus every registered
// `use:scrollable` pane
export type ScrollMap = Record<string, number>;

// one history entry's state — `{ d }` marks an entry as ours;
// `below` is the path of the entry under it (what back lands on)
interface EntryState {
  d: number;
  below?: string;
  scroll?: ScrollMap;
}

export interface HistoryShim {
  readonly state: unknown;
  readonly url: string;
  pushState(state: unknown, url: string): void;
  replaceState(state: unknown, url: string | null): void;
  back(): void;
}

export interface ScrollSink {
  // snapshot every scroll container by id
  capture(): ScrollMap;
  // true when the current content is tall enough for the offsets
  fits(map: ScrollMap): boolean;
  // one render cycle of the host app (tick + a frame)
  settled(): Promise<void>;
  // set every known container to its saved offset, missing → top
  apply(map: ScrollMap): void;
}

export interface Router {
  init(): void;
  navigate(path: string, opts?: { replace?: boolean }): void;
  // to the parent route: history.back() when the entry below is
  // exactly that parent, else a navigation to it
  goBack(parent: string): void;
  onPopState(): void;
}

const RESTORE_TIMEOUT = 1000;

export function createRouter(deps: {
  history: HistoryShim;
  scroll: ScrollSink;
  onRoute(route: Route): void;
  now?(): number;
}): Router {
  const now = deps.now ?? Date.now;
  let depth = 0;
  let restoreRun = 0;

  function entryState(state: unknown): EntryState | null {
    if (state === null || typeof state !== "object") return null;
    return typeof (state as EntryState).d === "number" ? (state as EntryState) : null;
  }

  // a back/forward hop can land before the new page has content:
  // wait one render cycle, retry while the target does not fit,
  // give up after ~1 s
  async function restore(map: ScrollMap): Promise<void> {
    const run = ++restoreRun;
    const deadline = now() + RESTORE_TIMEOUT;
    do {
      await deps.scroll.settled();
      if (run !== restoreRun) return;
      if (deps.scroll.fits(map)) break;
    } while (now() < deadline);
    deps.scroll.apply(map);
  }

  // remember where the entry being left was scrolled to
  function saveScroll(): void {
    const below = entryState(deps.history.state)?.below;
    deps.history.replaceState({ d: depth, below, scroll: deps.scroll.capture() }, null);
  }

  // adopt the current URL: unknown paths fold to the home route
  function report(): Route {
    const match = matchRoute(deps.history.url);
    if (!match) deps.history.replaceState({ d: depth }, "/");
    // a redirected URL (a legacy /system link) is rewritten in place
    else if (match.path !== deps.history.url) deps.history.replaceState({ d: depth }, match.path);
    const route = match ?? { ...HOME_ROUTE };
    deps.onRoute(route);
    return route;
  }
  function navigate(path: string, opts?: { replace?: boolean }): void {
    const match = matchRoute(path) ?? { ...HOME_ROUTE };
    // a redirected legacy path never adds a history entry — it
    // lands on its target in place
    const redirected = REDIRECTS[normalize(path)] !== undefined;
    if (redirected && match.path === deps.history.url) return;
    if (opts?.replace || redirected) {
      const below = entryState(deps.history.state)?.below;
      deps.history.replaceState({ d: depth, below, scroll: {} }, match.path);
    } else {
      if (match.path === deps.history.url) return;
      const from = deps.history.url;
      saveScroll();
      depth += 1;
      deps.history.pushState({ d: depth, below: from }, match.path);
    }
    deps.onRoute(match);
    deps.scroll.apply({}); // every new route starts at the top
  }

  return {
    init() {
      const existing = entryState(deps.history.state);
      depth = existing?.d ?? 0;
      // mark the current entry as ours (a reload keeps its state)
      deps.history.replaceState({ d: depth, scroll: existing?.scroll }, null);
      report();
      // a reload of a deep link replays its last position
      if (existing?.scroll) void restore(existing.scroll);
    },

    navigate,

    // back to `parent`: history.back() only when the entry below
    // IS the parent (keeps its scroll position); otherwise — a
    // deep link, or sidebar hops between sections on a wide
    // screen — navigate to it
    goBack(parent) {
      const below = entryState(deps.history.state)?.below;
      if (depth > 0 && below !== undefined && normalize(below) === normalize(parent)) {
        deps.history.back();
      } else {
        navigate(parent);
      }
    },

    onPopState() {
      const state = entryState(deps.history.state);
      if (state) depth = state.d;
      report();
      void restore(state?.scroll ?? {});
    },
  };
}
