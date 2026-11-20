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
if (typeof window !== "undefined" && typeof window.matchMedia !== "function") {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

// jsdom doesn't implement ResizeObserver — provide a no-op stub so hooks that
// observe layout (e.g. useResponsive) can mount in tests. Suites that need to
// assert on callback behavior override `global.ResizeObserver` themselves.
if (typeof global.ResizeObserver === "undefined") {
  global.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
}

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
// Realtime snapshot / event-stream helpers
// ---------------------------------------------------------------------------
// Tests need a deterministic clock and a controllable websocket so they can
// simulate sequence gaps, out-of-order delivery, reconnect, and retention
// expiry without real timers or network access.

if (typeof global.WebSocket === "undefined") {
  class TestWebSocket {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSING = 2;
    static readonly CLOSED = 3;

    readonly url: string;
    readyState = TestWebSocket.CONNECTING;
    onopen: ((ev: unknown) => void) | null = null;
    onclose: ((ev: unknown) => void) | null = null;
    onerror: ((ev: unknown) => void) | null = null;
    onmessage: ((ev: { data: string }) => void) | null = null;
    sent: string[] = [];

    constructor(url: string) {
      this.url = url;
    }

    send(data: string): void {
      this.sent.push(data);
    }

    close(): void {
      this.readyState = TestWebSocket.CLOSED;
      this.onclose?.({});
    }

    // Test-only helpers.
    open(): void {
      this.readyState = TestWebSocket.OPEN;
      this.onopen?.({});
    }

    emit(payload: unknown): void {
      this.onmessage?.({ data: JSON.stringify(payload) });
    }
  }
  (globalThis as { WebSocket?: unknown }).WebSocket = TestWebSocket;
}

// ---------------------------------------------------------------------------
// Offline command queue helpers
// ---------------------------------------------------------------------------
// Provide a minimal in-memory localStorage when jsdom omits it (e.g. in node
// test environments) so the scoped offline queue can persist and purge commands.

if (typeof global.localStorage === "undefined") {
  class MemoryStorage implements Storage {
    private store = new Map<string, string>();

    get length(): number {
      return this.store.size;
    }

    clear(): void {
      this.store.clear();
    }

    getItem(key: string): string | null {
      return this.store.has(key) ? (this.store.get(key) as string) : null;
    }

    key(index: number): string | null {
      return Array.from(this.store.keys())[index] ?? null;
    }

    removeItem(key: string): void {
      this.store.delete(key);
    }

    setItem(key: string, value: string): void {
      this.store.set(key, String(value));
    }
  }
  (globalThis as { localStorage?: unknown }).localStorage = new MemoryStorage();
}

// ---------------------------------------------------------------------------
// Return-destination validator helpers
// ---------------------------------------------------------------------------
// jsdom's default origin is http://localhost; pin it so redirect validation
// tests have a stable, local-only base to compare against.

if (typeof window !== "undefined" && !window.location.origin) {
  Object.defineProperty(window, "location", {
    writable: true,
    value: { ...window.location, origin: "http://localhost" },
  });
}

// Reset per-test state that leaks between suites: queued offline commands,
// pending optimistic mutations, and any stubbed realtime sockets.
afterEach(() => {
  if (typeof global.localStorage !== "undefined") {
    global.localStorage.clear();
  }
  jest.clearAllMocks();
});
