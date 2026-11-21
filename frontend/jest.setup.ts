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

// jsdom doesn't implement crypto.randomUUID — provide a deterministic fallback
// so error boundaries can mint correlation IDs in tests.
if (typeof globalThis.crypto === "undefined") {
  (globalThis as { crypto?: unknown }).crypto = {};
}
if (typeof globalThis.crypto.randomUUID !== "function") {
  let uuidCounter = 0;
  Object.defineProperty(globalThis.crypto, "randomUUID", {
    writable: true,
    configurable: true,
    value: () => {
      uuidCounter += 1;
      const suffix = uuidCounter.toString(16).padStart(12, "0");
      return `00000000-0000-4000-8000-${suffix}`;
    },
  });
}

// Provide a fetch stub for analytics API route tests. Suites that need to
// assert on requests override `global.fetch` themselves.
if (typeof global.fetch === "undefined") {
  global.fetch = jest.fn(async () =>
    new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  ) as unknown as typeof fetch;
}

// Reset consent and session storage between tests so consent-transition and
// logout flows start from a clean, deterministic state.
beforeEach(() => {
  if (typeof window !== "undefined") {
    try {
      window.localStorage.clear();
      window.sessionStorage.clear();
    } catch {
      // Storage may be unavailable in some jsdom configurations.
    }
  }
});

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

// ---------------------------------------------------------------------------
// Cross-tab invalidation primitives
// ---------------------------------------------------------------------------
// jsdom does not implement BroadcastChannel. Provide an in-memory hub so tests
// can simulate simultaneous tabs (each channel instance is a "tab"), closed-tab
// delivery (closing a channel stops delivery to it), and environments without
// the primitive (delete `globalThis.BroadcastChannel` in the suite).
//
// The hub mirrors the browser semantics the invalidation protocol relies on:
//  - messages are delivered asynchronously to every *other* channel with the
//    same name (never to the sender),
//  - structured-clone-like copying so receivers can't mutate sender payloads,
//  - delivery to closed channels is silently dropped.
export interface TestBroadcastChannelMessageEvent {
  data: unknown;
}

type TestBroadcastChannelListener = (event: TestBroadcastChannelMessageEvent) => void;

const broadcastChannelRegistry = new Map<string, Set<TestBroadcastChannel>>();

function cloneBroadcastPayload(data: unknown): unknown {
  if (typeof structuredClone === "function") {
    try {
      return structuredClone(data);
    } catch {
      // Fall through to JSON cloning for non-cloneable payloads.
    }
  }
  try {
    return JSON.parse(JSON.stringify(data));
  } catch {
    return data;
  }
}

export class TestBroadcastChannel {
  readonly name: string;
  onmessage: TestBroadcastChannelListener | null = null;
  onmessageerror: ((event: unknown) => void) | null = null;
  private listeners = new Set<TestBroadcastChannelListener>();
  private closed = false;

  constructor(name: string) {
    this.name = String(name);
    let peers = broadcastChannelRegistry.get(this.name);
    if (!peers) {
      peers = new Set();
      broadcastChannelRegistry.set(this.name, peers);
    }
    peers.add(this);
  }

  postMessage(data: unknown): void {
    if (this.closed) {
      throw new DOMException("Channel is closed", "InvalidStateError");
    }
    const peers = broadcastChannelRegistry.get(this.name);
    if (!peers) return;
    const payload = cloneBroadcastPayload(data);
    for (const peer of Array.from(peers)) {
      if (peer === this || peer.closed) continue;
      // Deliver asynchronously, like a real BroadcastChannel.
      setTimeout(() => {
        if (peer.closed) return;
        const event: TestBroadcastChannelMessageEvent = { data: cloneBroadcastPayload(payload) };
        try {
          peer.onmessage?.(event);
        } catch {
          // A throwing listener must not break delivery to other tabs.
        }
        for (const listener of Array.from(peer.listeners)) {
          try {
            listener(event);
          } catch {
            // Ignore listener failures; other listeners still receive the event.
          }
        }
      }, 0);
    }
  }

  addEventListener(type: string, listener: TestBroadcastChannelListener): void {
    if (type === "message") this.listeners.add(listener);
  }

  removeEventListener(type: string, listener: TestBroadcastChannelListener): void {
    if (type === "message") this.listeners.delete(listener);
  }

  close(): void {
    this.closed = true;
    this.listeners.clear();
    this.onmessage = null;
    const peers = broadcastChannelRegistry.get(this.name);
    peers?.delete(this);
    if (peers && peers.size === 0) broadcastChannelRegistry.delete(this.name);
  }
}

/** Test helper: drop every registered channel (call in afterEach). */
export function resetBroadcastChannels(): void {
  for (const peers of broadcastChannelRegistry.values()) {
    for (const peer of peers) peer.close();
  }
  broadcastChannelRegistry.clear();
}

if (typeof globalThis.BroadcastChannel === "undefined") {
  (globalThis as { BroadcastChannel?: unknown }).BroadcastChannel = TestBroadcastChannel;
}

// jsdom does not implement the Page Visibility API. Provide a controllable stub
// so invalidation logic that pauses while a tab is hidden can be exercised.
if (typeof document !== "undefined" && typeof document.hidden === "undefined") {
  let hidden = false;
  Object.defineProperty(document, "hidden", {
    configurable: true,
    get: () => hidden,
  });
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => (hidden ? "hidden" : "visible"),
  });
  (document as unknown as { __setHidden: (value: boolean) => void }).__setHidden = (value: boolean) => {
    hidden = value;
    document.dispatchEvent(new Event("visibilitychange"));
  };
}

// jsdom does not implement the StorageEvent constructor in all versions; make
// sure the fallback broadcast path (localStorage events) can be simulated.
if (typeof window !== "undefined" && typeof window.StorageEvent === "undefined") {
  class TestStorageEvent extends Event {
    readonly key: string | null;
    readonly oldValue: string | null;
    readonly newValue: string | null;
    readonly url: string | null;
    readonly storageArea: Storage | null;

    constructor(
      type: string,
      init: { key?: string | null; oldValue?: string | null; newValue?: string | null; url?: string | null; storageArea?: Storage | null } = {},
    ) {
      super(type);
      this.key = init.key ?? null;
      this.oldValue = init.oldValue ?? null;
      this.newValue = init.newValue ?? null;
      this.url = init.url ?? null;
      this.storageArea = init.storageArea ?? null;
    }
  }
  (window as unknown as { StorageEvent?: unknown }).StorageEvent = TestStorageEvent;
}
