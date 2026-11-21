import { describe, expect, test } from "bun:test";
import {
  createRouter,
  HOME_ROUTE,
  matchRoute,
  RESERVED_PATHS,
  type HistoryShim,
  type Route,
  type ScrollMap,
  type ScrollSink,
} from "./router";

// ── fakes ──

type Entry = { state: unknown; url: string };

class FakeHistory implements HistoryShim {
  entries: Entry[] = [];
  index = 0;
  onPopState: () => void = () => {};

  constructor(initialUrl: string) {
    this.entries.push({ state: null, url: initialUrl });
  }

  get state(): unknown {
    return this.entries[this.index]?.state ?? null;
  }
  get url(): string {
    return this.entries[this.index]?.url ?? "/";
  }

  pushState(state: unknown, url: string): void {
    this.entries.length = this.index + 1;
    this.entries.push({ state, url });
    this.index += 1;
  }

  replaceState(state: unknown, url: string | null): void {
    if (this.index < 0) {
      this.entries.push({ state, url: url ?? "/" });
      this.index = 0;
      return;
    }
    this.entries[this.index] = { state, url: url ?? this.entries[this.index].url };
  }

  back(): void {
    this.index -= 1;
    this.onPopState();
  }
}

function fakeScroll(offsets: ScrollMap = { window: 0 }) {
  const applied: ScrollMap[] = [];
  const sink: ScrollSink = {
    capture: () => ({ ...offsets }),
    fits: () => true,
    settled: async () => {},
    apply: (map) => applied.push({ ...map }),
  };
  return { sink, applied };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 5));

const last = <T>(array: T[]): T => array[array.length - 1];

function setup(pathname: string, sink: ScrollSink = fakeScroll().sink) {
  const history = new FakeHistory(pathname);
  const routes: Route[] = [];
  const router = createRouter({
    history,
    scroll: sink,
    onRoute: (route) => routes.push({ ...route }),
  });
  history.onPopState = () => router.onPopState();
  router.init();
  return { history, routes, router };
}

// ── matching ──

describe("matchRoute", () => {
  test("the app routes with their params", () => {
    expect(matchRoute("/")).toEqual({ page: "remote", path: "/", section: null, skin: null });
    expect(matchRoute("/settings")).toEqual({
      page: "settings",
      path: "/settings",
      section: null,
      skin: null,
    });
    expect(matchRoute("/settings/info")).toEqual({
      page: "settings",
      path: "/settings/info",
      section: "info",
      skin: null,
    });
    expect(matchRoute("/settings/vu")).toEqual({
      page: "settings",
      path: "/settings/vu",
      section: "vu",
      skin: null,
    });
    expect(matchRoute("/settings/vu/night")).toEqual({
      page: "settings",
      path: "/settings/vu/night",
      section: "vu",
      skin: "night",
    });
  });

  test("the device sections are plain settings sections", () => {
    for (const section of ["audio", "system"]) {
      expect(matchRoute(`/settings/${section}`)).toEqual({
        page: "settings",
        path: `/settings/${section}`,
        section,
        skin: null,
      });
    }
  });

  test("the legacy /system path redirects to the info section", () => {
    expect(matchRoute("/system")).toEqual(matchRoute("/settings/info"));
  });

  test("the wifi setup route matches", () => {
    expect(matchRoute("/setup/wifi")).toEqual({
      page: "setup",
      path: "/setup/wifi",
      section: null,
      skin: null,
    });
    expect(matchRoute("/setup/wifi/")).toEqual(matchRoute("/setup/wifi"));
  });

  test("the captive-portal probe paths are backend-only, never routes", () => {
    for (const path of [
      "/generate_204",
      "/gen_204",
      "/hotspot-detect.html",
      "/library/test/success.html",
      "/connecttest.txt",
      "/ncsi.txt",
    ]) {
      expect(matchRoute(path)).toBeNull();
    }
  });

  test("trailing slashes fold away", () => {
    expect(matchRoute("/settings/")).toEqual(matchRoute("/settings"));
    expect(matchRoute("/system/")).toEqual(matchRoute("/system"));
    expect(matchRoute("/settings/vu/night/")).toEqual(matchRoute("/settings/vu/night"));
  });

  test("unknown paths do not match", () => {
    expect(matchRoute("/foo")).toBeNull();
    expect(matchRoute("/SETTINGS")).toBeNull();
    expect(matchRoute("/settings/vu/night/extra")).toBeNull();
    expect(matchRoute("")).toEqual({ ...HOME_ROUTE });
  });

  test("reserved backend paths are never app routes", () => {
    expect(RESERVED_PATHS).toEqual([
      "/ws",
      "/pair",
      "/pair/status",
      "/cover",
      "/ota",
      "/vu-preview",
      "/clock-preview",
      "/diagnostics",
    ]);
    for (const path of RESERVED_PATHS) {
      expect(matchRoute(path)).toBeNull();
    }
  });
});

// ── navigation ──

describe("navigate", () => {
  test("push adds an entry and reports the route", () => {
    const { history, routes, router } = setup("/settings");
    expect(history.entries).toHaveLength(1);
    expect(routes[0]).toMatchObject({ page: "settings", section: null });

    router.navigate("/settings/vu");
    expect(history.entries).toHaveLength(2);
    expect(history.entries[1]).toMatchObject({ url: "/settings/vu", state: { d: 1 } });
    expect(routes[1]).toMatchObject({ page: "settings", section: "vu", skin: null });
  });

  test("replace swaps the current entry without adding history", () => {
    const { history, routes, router } = setup("/settings");
    router.navigate("/settings/vu", { replace: true });
    expect(history.entries).toHaveLength(1);
    expect(history.entries[0]).toMatchObject({ url: "/settings/vu", state: { d: 0 } });
    expect(routes[1]).toMatchObject({ section: "vu", skin: null });
  });

  test("the same path is a no-op", () => {
    const { history, router } = setup("/settings");
    router.navigate("/settings");
    expect(history.entries).toHaveLength(1);
  });

  test("a redirected path navigates in place, not as a new entry", () => {
    const { history, routes, router } = setup("/");
    router.navigate("/system");
    expect(history.entries).toHaveLength(1);
    expect(history.entries[0]).toMatchObject({ url: "/settings/info", state: { d: 0 } });
    expect(last(routes)).toMatchObject({ page: "settings", section: "info" });
  });

  test("a redirected path already on its target is a no-op", () => {
    const { history, routes, router } = setup("/settings/info");
    router.navigate("/system");
    expect(history.entries).toHaveLength(1);
    expect(routes).toHaveLength(1);
  });

  test("unknown paths fold to the home route", () => {
    const { history, routes, router } = setup("/settings");
    router.navigate("/nope");
    expect(history.entries[1]?.url).toBe("/");
    expect(last(routes)).toMatchObject({ page: "remote", path: "/" });
  });

  test("an unknown initial path is replaced with /", () => {
    const { history, routes } = setup("/nope");
    expect(history.entries[0]?.url).toBe("/");
    expect(routes[0]).toMatchObject({ page: "remote", path: "/" });
  });

  test("an unknown path on popstate is replaced with /", () => {
    const { history, routes, router } = setup("/settings");
    router.navigate("/settings/info");

    history.entries[0].url = "/ws";
    history.back();
    expect(history.entries[0]?.url).toBe("/");
    expect(last(routes)).toMatchObject({ page: "remote" });
  });

  test("a legacy /system entry on popstate is rewritten to its target", () => {
    const { history, routes, router } = setup("/settings");
    router.navigate("/settings/info");

    history.entries[0].url = "/system";
    history.back();
    expect(history.entries[0]?.url).toBe("/settings/info");
    expect(last(routes)).toMatchObject({ page: "settings", section: "info" });
  });

  test("a /system deep link is adopted as the info section", () => {
    const { history, routes } = setup("/system");
    expect(history.entries[0]?.url).toBe("/settings/info");
    expect(routes[0]).toMatchObject({ page: "settings", section: "info" });
  });
});

describe("back", () => {
  test("goBack from a device section lands on the list, from a deep link too", () => {
    const { history, routes, router } = setup("/settings");
    router.navigate("/settings/system");
    router.goBack("/settings");
    expect(history.index).toBe(0);
    expect(last(routes)).toMatchObject({ page: "settings", section: null });

    const deepLink = setup("/settings/audio");
    deepLink.router.goBack("/settings");
    expect(last(deepLink.routes)).toMatchObject({
      page: "settings",
      path: "/settings",
      section: null,
    });
  });

  test("the wifi setup returns to the system section", () => {
    const { history, routes, router } = setup("/settings/system");
    router.navigate("/setup/wifi");
    router.goBack("/settings/system");
    expect(history.index).toBe(0);
    expect(last(routes)).toMatchObject({ page: "settings", section: "system" });
  });

  test("popstate restores the previous route", () => {
    const { history, routes, router } = setup("/settings");
    router.navigate("/settings/info");
    history.back();
    expect(last(routes)).toMatchObject({ page: "settings", path: "/settings" });
  });

  test("popstate keeps the depth stored in the entry", () => {
    const { history, router } = setup("/settings/vu/night");
    router.navigate("/settings/info");
    history.back(); // back onto the deep link: its entry is depth 0
    router.goBack("/settings/vu");
    // the fallback pushed — a stale depth would have gone history.back
    expect(history.index).toBe(1);
    expect(history.entries[1]?.url).toBe("/settings/vu");
  });

  test("goBack stays in history after in-app navigation", () => {
    const { history, routes, router } = setup("/");
    router.navigate("/settings");
    router.goBack("/");
    expect(history.entries).toHaveLength(2);
    expect(history.index).toBe(0);
    expect(last(routes)).toMatchObject({ page: "remote", path: "/" });
  });

  test("goBack goes to its target, not just one entry back", () => {
    // desktop: the sidebar hops between sections, then the top-bar
    // back button means the remote — not the previous section
    const { history, routes, router } = setup("/");
    router.navigate("/settings");
    router.navigate("/settings/vu");
    router.navigate("/settings/notifications");
    router.goBack("/");
    expect(last(routes)).toMatchObject({ page: "remote", path: "/" });
    expect(history.entries[history.index]?.url).toBe("/");
  });

  test("goBack uses history.back when the entry below is the target", () => {
    // keeps the scroll position the parent was left at
    const { history, router } = setup("/settings/vu");
    router.navigate("/settings/vu/night");
    router.goBack("/settings/vu");
    expect(history.entries).toHaveLength(2);
    expect(history.index).toBe(0);
  });

  test("goBack falls back to the parent route on a direct deep link", () => {
    const { history, routes, router } = setup("/settings/vu/night");
    router.goBack("/settings/vu");
    expect(history.entries).toHaveLength(2);
    expect(history.entries[1]?.url).toBe("/settings/vu");
    expect(last(routes)).toMatchObject({ section: "vu", skin: null });
  });
});

// ── scroll ──

describe("scroll", () => {
  test("leaving an entry saves its position, coming back restores it", async () => {
    const scroll = fakeScroll({ window: 320, "settings-list": 40 });
    const { history, router } = setup("/settings", scroll.sink);
    router.navigate("/settings/info");
    expect(history.entries[0]?.state).toMatchObject({
      d: 0,
      scroll: { window: 320, "settings-list": 40 },
    });

    history.back();
    await flush();
    expect(last(scroll.applied)).toEqual({ window: 320, "settings-list": 40 });
  });

  test("new routes start at the top", () => {
    const scroll = fakeScroll({ window: 320 });
    const { router } = setup("/settings", scroll.sink);
    router.navigate("/settings/info");
    expect(last(scroll.applied)).toEqual({});
  });

  test("forward onto an entry left via browser back starts at the top", async () => {
    const scroll = fakeScroll({ window: 320 });
    const { history, router } = setup("/settings", scroll.sink);
    router.navigate("/settings/info");
    history.back(); // leaves the info entry via back: nothing saved on it
    await flush();
    expect(last(scroll.applied)).toEqual({ window: 320 });
    history.index += 1; // browser forward onto the info entry
    history.onPopState();
    await flush();
    expect(last(scroll.applied)).toEqual({});
  });

  test("restore waits until the content is tall enough", async () => {
    let tall = false;
    const applied: ScrollMap[] = [];
    const sink: ScrollSink = {
      capture: () => ({ window: 400 }),
      fits: () => tall,
      settled: async () => {
        tall = true;
      },
      apply: (map) => applied.push({ ...map }),
    };
    const { history, router } = setup("/settings", sink);
    router.navigate("/settings/info");
    history.back();
    await flush();
    // applied only after one render cycle made the content fit
    expect(last(applied)).toEqual({ window: 400 });
  });

  test("restore gives up after about a second", async () => {
    let clock = 0;
    let settles = 0;
    const applied: ScrollMap[] = [];
    const sink: ScrollSink = {
      capture: () => ({ window: 400 }),
      fits: () => false,
      settled: async () => {
        clock += 500;
        settles += 1;
      },
      apply: (map) => applied.push({ ...map }),
    };
    const history = new FakeHistory("/settings");
    const router = createRouter({
      history,
      scroll: sink,
      now: () => clock,
      onRoute: () => {},
    });
    history.onPopState = () => router.onPopState();
    router.init();
    router.navigate("/settings/info");
    clock = 0;
    history.back();
    await flush();
    expect(last(applied)).toEqual({ window: 400 });
    expect(settles).toBeLessThanOrEqual(3);
  });

  test("a reload restores the position saved in history.state", async () => {
    const scroll = fakeScroll({ window: 0 });
    const history = new FakeHistory("/settings");
    history.entries[0] = {
      state: { d: 1, scroll: { window: 250, "settings-list": 30 } },
      url: "/settings",
    };
    const routes: Route[] = [];
    const router = createRouter({
      history,
      scroll: scroll.sink,
      onRoute: (route) => routes.push({ ...route }),
    });
    router.init();
    expect(routes[0]).toMatchObject({ page: "settings" });
    await flush();
    expect(scroll.applied).toEqual([{ window: 250, "settings-list": 30 }]);
  });
});
