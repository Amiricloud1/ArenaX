import "@testing-library/jest-dom";
import { TextDecoder, TextEncoder } from "util";

// jsdom doesn't provide the Fetch API's Response — route handlers under test
// (e.g. the OG image 404s) construct one directly.
if (typeof (globalThis as { Response?: unknown }).Response === "undefined") {
  class TestResponse {
    status: number;
    ok: boolean;
    headers: Headers;
    body: unknown;

    constructor(body?: unknown, init: { status?: number; headers?: Record<string, string> } = {}) {
      this.status = init.status ?? 200;
      this.ok = this.status >= 200 && this.status < 300;
      this.headers = new Headers(init.headers);
      this.body = body;
    }

    async text(): Promise<string> {
      return typeof this.body === "string" ? this.body : "";
    }

    async json(): Promise<unknown> {
      return JSON.parse(await this.text());
    }

    static json(data: unknown, init: { status?: number } = {}): TestResponse {
      return new Response(JSON.stringify(data), {
        status: init.status ?? 200,
        headers: { "content-type": "application/json" },
      });
    }
  }
  (globalThis as { Response?: unknown }).Response = TestResponse;
}

if (typeof global.TextEncoder === "undefined") {
  // Required by stellar-sdk in Jest runtime.
  global.TextEncoder = TextEncoder as unknown as typeof global.TextEncoder;
}

if (typeof global.TextDecoder === "undefined") {
  global.TextDecoder = TextDecoder as unknown as typeof global.TextDecoder;
}

// jsdom doesn't implement scrollIntoView — stub it so focus helpers don't crash.
if (typeof window !== "undefined" && typeof Element !== "undefined" &&
    !Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

// jsdom doesn't implement matchMedia — provide a minimal stub so components
// that probe prefers-reduced-motion / prefers-contrast don't crash in tests.
// The stub tracks listeners and exposes `__setMatches` so reduced-motion and
// RTL suites can flip media queries at runtime and assert on re-render.
type MediaQueryListener = (event: MediaQueryListEvent) => void;

interface TestMediaQueryList extends MediaQueryList {
  __setMatches: (matches: boolean) => void;
}

const mediaQueryLists = new Map<string, TestMediaQueryList>();

function createMediaQueryList(query: string): TestMediaQueryList {
  const listeners = new Set<MediaQueryListener>();
  const mql = {
    matches: false,
    media: query,
    onchange: null as MediaQueryListener | null,
    addListener: (listener: MediaQueryListener) => listeners.add(listener),
    removeListener: (listener: MediaQueryListener) => listeners.delete(listener),
    addEventListener: (_type: string, listener: MediaQueryListener) => listeners.add(listener),
    removeEventListener: (_type: string, listener: MediaQueryListener) => listeners.delete(listener),
    dispatchEvent: () => false,
    __setMatches(matches: boolean) {
      mql.matches = matches;
      const event = { matches, media: query } as MediaQueryListEvent;
      mql.onchange?.(event);
      listeners.forEach((listener) => listener(event));
    },
  };
  return mql as unknown as TestMediaQueryList;
}

if (typeof window !== "undefined" && typeof window.matchMedia !== "function") {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => {
      const existing = mediaQueryLists.get(query);
      if (existing) return existing;
      const created = createMediaQueryList(query);
      mediaQueryLists.set(query, created);
      return created;
    },
  });
}

// Helper for tests that need to simulate prefers-reduced-motion / RTL media
// queries. Safe to call even when a suite has replaced window.matchMedia.
(globalThis as { setMediaQueryMatches?: (query: string, matches: boolean) => void })
  .setMediaQueryMatches = (query: string, matches: boolean) => {
  const mql = mediaQueryLists.get(query);
  if (mql) {
    mql.__setMatches(matches);
  } else if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
    const created = createMediaQueryList(query);
    mediaQueryLists.set(query, created);
    created.__setMatches(matches);
  }
};

// jsdom doesn't implement ResizeObserver — provide a no-op stub so hooks that
// observe layout (e.g. useResponsive) can mount in tests. Suites that need to
// assert on callback behavior override `global.ResizeObserver` themselves.
// The stub records observed targets so RTL layout tests can assert on them.
if (typeof global.ResizeObserver === "undefined") {
  global.ResizeObserver = class {
    static instances: Array<{ targets: Set<Element> }> = [];
    targets = new Set<Element>();

    constructor() {
      (global.ResizeObserver as unknown as { instances: Array<{ targets: Set<Element> }> })
        .instances.push(this);
    }

    observe(target: Element): void {
      this.targets.add(target);
    }

    unobserve(target: Element): void {
      this.targets.delete(target);
    }

    disconnect(): void {
      this.targets.clear();
    }
  } as unknown as typeof ResizeObserver;
}

// jsdom doesn't implement the Gamepad API — provide a controllable stub so
// gamepad navigation tests can connect/disconnect pads and drive button and
// axis state. Tests use `__setGamepads` / `__emitGamepadConnected` helpers.
interface TestGamepadButton {
  pressed: boolean;
  touched: boolean;
  value: number;
}

interface TestGamepad {
  id: string;
  index: number;
  connected: boolean;
  mapping: string;
  timestamp: number;
  axes: number[];
  buttons: TestGamepadButton[];
  vibrationActuator: null;
}

function createTestGamepad(index: number, id = `Test Pad ${index}`): TestGamepad {
  return {
    id,
    index,
    connected: true,
    mapping: "standard",
    timestamp: 0,
    axes: [0, 0, 0, 0],
    buttons: Array.from({ length: 17 }, () => ({ pressed: false, touched: false, value: 0 })),
    vibrationActuator: null,
  };
}

const connectedGamepads = new Map<number, TestGamepad>();

if (typeof navigator !== "undefined" && typeof navigator.getGamepads !== "function") {
  Object.defineProperty(navigator, "getGamepads", {
    configurable: true,
    writable: true,
    value: () => {
      const slots: Array<TestGamepad | null> = [null, null, null, null];
      connectedGamepads.forEach((pad, index) => {
        if (index < slots.length) slots[index] = pad;
      });
      return slots;
    },
  });
}

(globalThis as {
  __setGamepads?: (pads: TestGamepad[]) => void;
  __emitGamepadConnected?: (pad: TestGamepad) => void;
  __emitGamepadDisconnected?: (pad: TestGamepad) => void;
  __createTestGamepad?: (index: number, id?: string) => TestGamepad;
}).__createTestGamepad = createTestGamepad;

(globalThis as { __setGamepads?: (pads: TestGamepad[]) => void }).__setGamepads = (
  pads: TestGamepad[],
) => {
  connectedGamepads.clear();
  pads.forEach((pad) => connectedGamepads.set(pad.index, pad));
};

(globalThis as { __emitGamepadConnected?: (pad: TestGamepad) => void })
  .__emitGamepadConnected = (pad: TestGamepad) => {
  connectedGamepads.set(pad.index, pad);
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event("gamepadconnected"));
  }
};

(globalThis as { __emitGamepadDisconnected?: (pad: TestGamepad) => void })
  .__emitGamepadDisconnected = (pad: TestGamepad) => {
  connectedGamepads.delete(pad.index);
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event("gamepaddisconnected"));
  }
};

// Ensure Intl formatters used by locale-aware views (money, percentages,
// durations, dates, plurals) are available and deterministic in tests. Node
// ships full ICU, but guard against stripped-down runtimes and pin the
// timezone so date snapshots don't drift between machines.
if (typeof Intl === "undefined" || typeof Intl.NumberFormat !== "function") {
  throw new Error("Intl.NumberFormat is required for locale-aware tests");
}

if (typeof Intl.PluralRules !== "function") {
  throw new Error("Intl.PluralRules is required for locale-aware tests");
}

if (typeof Intl.RelativeTimeFormat !== "function") {
  throw new Error("Intl.RelativeTimeFormat is required for locale-aware tests");
}

if (typeof Intl.DateTimeFormat !== "function") {
  throw new Error("Intl.DateTimeFormat is required for locale-aware tests");
}

process.env.TZ = process.env.TZ ?? "UTC";

// Global next/navigation mock so components using useRouter, useSearchParams,
// or usePathname (such as MatchHistory) mount cleanly in tests without requiring
// every single test suite to manually declare the same mock.
jest.mock("next/navigation", () => ({
  useRouter: () => ({
    push: jest.fn(),
    replace: jest.fn(),
    prefetch: jest.fn(),
    back: jest.fn(),
    forward: jest.fn(),
    refresh: jest.fn(),
  }),
  usePathname: () => "/en/profile",
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({ locale: "en" }),
}));
