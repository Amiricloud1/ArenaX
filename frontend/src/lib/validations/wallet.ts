import { z } from "zod";

export const STELLAR_ADDRESS_REGEX = /^G[A-Z0-9]{55}$/;

export const withdrawSchema = z.object({
  asset: z.enum(["XLM", "USDC", "ARENAX"]),
  amount: z
    .string()
    .min(1, "Amount is required")
    .refine(
      (v) => {
        const n = Number(v);
        return Number.isFinite(n) && n > 0;
      },
      { message: "Enter a valid amount greater than 0" }
    ),
  destination: z
    .string()
    .min(1, "Destination address is required")
    .regex(
      STELLAR_ADDRESS_REGEX,
      "Destination must be a valid Stellar public key (starts with G, 56 chars)"
    ),
  memo: z.string().max(28, "Memo must be 28 characters or less").optional(),
});

export type WithdrawFormData = z.infer<typeof withdrawSchema>;

export const depositSchema = z.object({
  asset: z.enum(["XLM", "USDC", "ARENAX"]),
  amount: z
    .string()
    .optional()
    .refine(
      (v) => {
        if (!v || v === "0" || v === "") return true;
        const n = Number(v);
        return Number.isFinite(n) && n >= 0;
      },
      { message: "Enter a valid amount" }
    ),
});

export type DepositFormData = z.infer<typeof depositSchema>;

/**
 * Wallet session state machine.
 *
 * A connected wallet can change account or network while a transaction flow is
 * open, which would otherwise cause the user to sign for the wrong context.
 * This module tracks the live wallet context (account + network), captures the
 * signing context at confirmation time, and invalidates pending quotes and
 * unsigned transaction previews whenever the live context transitions.
 *
 * It is framework-agnostic so it can be driven by a React hook, a store, or
 * tests. The hook wrapper (`useWalletSession`) lives in
 * `frontend/src/hooks/useWalletSession.ts` and delegates to this machine.
 */

export type WalletNetwork = "PUBLIC" | "TESTNET" | "FUTURENET" | "STANDALONE" | string;

export interface WalletContext {
  /** Stellar public key of the connected account, or null when disconnected. */
  account: string | null;
  /** Network passphrase identifier, or null when disconnected. */
  network: WalletNetwork | null;
}

export interface CapturedSigningContext {
  account: string;
  network: WalletNetwork;
  /** Monotonic id so consumers can distinguish successive confirmations. */
  capturedAt: number;
}

export type WalletSessionEvent =
  | { type: "connect"; account: string; network: WalletNetwork }
  | { type: "accountChanged"; account: string }
  | { type: "networkChanged"; network: WalletNetwork }
  | { type: "disconnect" }
  | { type: "reconnect"; account: string; network: WalletNetwork }
  | { type: "confirm" }
  | { type: "renewConsent" }
  | { type: "clearPending" };

export interface WalletSessionState {
  /** Live wallet context as reported by the wallet provider. */
  live: WalletContext;
  /** True when a wallet is currently connected. */
  connected: boolean;
  /** Signing context captured at the last confirmation, if any. */
  captured: CapturedSigningContext | null;
  /**
   * True when the live context differs from the captured signing context.
   * Consumers must require renewed consent before submitting.
   */
  requiresRenewedConsent: boolean;
  /**
   * Incremented on every context transition. Consumers can use this as a
   * dependency to invalidate pending quotes and unsigned previews.
   */
  contextVersion: number;
  /** Reason for the most recent invalidation, for UI messaging. */
  lastInvalidationReason: WalletInvalidationReason | null;
}

export type WalletInvalidationReason =
  | "accountChanged"
  | "networkChanged"
  | "disconnect"
  | "reconnect";

export interface WalletSessionCallbacks {
  /** Called whenever pending quotes should be discarded. */
  onInvalidatePendingQuotes?: (reason: WalletInvalidationReason) => void;
  /** Called whenever an unsigned transaction preview should be discarded. */
  onInvalidateUnsignedPreview?: (reason: WalletInvalidationReason) => void;
  /** Called when renewed consent is required before submission. */
  onRequiresRenewedConsent?: (captured: CapturedSigningContext, live: WalletContext) => void;
}

export const initialWalletSessionState: WalletSessionState = {
  live: { account: null, network: null },
  connected: false,
  captured: null,
  requiresRenewedConsent: false,
  contextVersion: 0,
  lastInvalidationReason: null,
};

function contextsMatch(
  captured: CapturedSigningContext | null,
  live: WalletContext
): boolean {
  if (!captured) return true;
  return captured.account === live.account && captured.network === live.network;
}

/**
 * Pure reducer for the wallet session state machine. Exported for testing and
 * for consumers that manage state outside React.
 */
export function walletSessionReducer(
  state: WalletSessionState,
  event: WalletSessionEvent
): WalletSessionState {
  switch (event.type) {
    case "connect": {
      const live: WalletContext = { account: event.account, network: event.network };
      const changed =
        state.live.account !== live.account || state.live.network !== live.network;
      return {
        ...state,
        live,
        connected: true,
        contextVersion: changed ? state.contextVersion + 1 : state.contextVersion,
        requiresRenewedConsent: !contextsMatch(state.captured, live),
        lastInvalidationReason: changed ? state.lastInvalidationReason : null,
      };
    }
    case "accountChanged": {
      const live: WalletContext = { ...state.live, account: event.account };
      return {
        ...state,
        live,
        connected: true,
        captured: state.captured,
        contextVersion: state.contextVersion + 1,
        requiresRenewedConsent: !contextsMatch(state.captured, live),
        lastInvalidationReason: "accountChanged",
      };
    }
    case "networkChanged": {
      const live: WalletContext = { ...state.live, network: event.network };
      return {
        ...state,
        live,
        connected: true,
        captured: state.captured,
        contextVersion: state.contextVersion + 1,
        requiresRenewedConsent: !contextsMatch(state.captured, live),
        lastInvalidationReason: "networkChanged",
      };
    }
    case "disconnect": {
      const live: WalletContext = { account: null, network: null };
      return {
        ...state,
        live,
        connected: false,
        captured: state.captured,
        contextVersion: state.contextVersion + 1,
        requiresRenewedConsent: !contextsMatch(state.captured, live),
        lastInvalidationReason: "disconnect",
      };
    }
    case "reconnect": {
      const live: WalletContext = { account: event.account, network: event.network };
      return {
        ...state,
        live,
        connected: true,
        captured: state.captured,
        contextVersion: state.contextVersion + 1,
        requiresRenewedConsent: !contextsMatch(state.captured, live),
        lastInvalidationReason: "reconnect",
      };
    }
    case "confirm": {
      if (!state.live.account || !state.live.network) {
        // Cannot confirm without a live context.
        return state;
      }
      const captured: CapturedSigningContext = {
        account: state.live.account,
        network: state.live.network,
        capturedAt: Date.now(),
      };
      return {
        ...state,
        captured,
        requiresRenewedConsent: false,
      };
    }
    case "renewConsent": {
      if (!state.live.account || !state.live.network) {
        return state;
      }
      const captured: CapturedSigningContext = {
        account: state.live.account,
        network: state.live.network,
        capturedAt: Date.now(),
      };
      return {
        ...state,
        captured,
        requiresRenewedConsent: false,
      };
    }
    case "clearPending": {
      return {
        ...state,
        contextVersion: state.contextVersion + 1,
      };
    }
    default:
      return state;
  }
}

/**
 * Returns true when a submission may proceed: the live context must match the
 * captured signing context. If it does not, renewed consent is required.
 */
export function canSubmitWithCapturedContext(state: WalletSessionState): boolean {
  if (!state.connected) return false;
  if (!state.captured) return false;
  return contextsMatch(state.captured, state.live);
}

/**
 * Imperative wallet session controller. Subscribes to wallet provider events
 * (account/network change, disconnect, reconnect) and drives the reducer,
 * invoking callbacks to invalidate pending quotes and unsigned previews.
 */
export class WalletSessionController {
  private state: WalletSessionState;
  private listeners = new Set<(state: WalletSessionState) => void>();
  private callbacks: WalletSessionCallbacks;

  constructor(
    initialState: WalletSessionState = initialWalletSessionState,
    callbacks: WalletSessionCallbacks = {}
  ) {
    this.state = initialState;
    this.callbacks = callbacks;
  }

  getState(): WalletSessionState {
    return this.state;
  }

  subscribe(listener: (state: WalletSessionState) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private emit(next: WalletSessionState) {
    this.state = next;
    for (const listener of this.listeners) {
      listener(next);
    }
  }

  private invalidate(reason: WalletInvalidationReason) {
    this.callbacks.onInvalidatePendingQuotes?.(reason);
    this.callbacks.onInvalidateUnsignedPreview?.(reason);
  }

  dispatch(event: WalletSessionEvent): WalletSessionState {
    const prev = this.state;
    const next = walletSessionReducer(prev, event);

    if (next === prev) {
      return prev;
    }

    // Invalidate pending work on any context transition.
    if (
      event.type === "accountChanged" ||
      event.type === "networkChanged" ||
      event.type === "disconnect" ||
      event.type === "reconnect"
    ) {
      this.invalidate(event.type);
    }

    if (next.requiresRenewedConsent && !prev.requiresRenewedConsent && next.captured) {
      this.callbacks.onRequiresRenewedConsent?.(next.captured, next.live);
    }

    this.emit(next);
    return next;
  }

  // Convenience wrappers for wallet provider event handlers.
  onConnect(account: string, network: WalletNetwork) {
    return this.dispatch({ type: "connect", account, network });
  }

  onAccountChanged(account: string) {
    return this.dispatch({ type: "accountChanged", account });
  }

  onNetworkChanged(network: WalletNetwork) {
    return this.dispatch({ type: "networkChanged", network });
  }

  onDisconnect() {
    return this.dispatch({ type: "disconnect" });
  }

  onReconnect(account: string, network: WalletNetwork) {
    return this.dispatch({ type: "reconnect", account, network });
  }

  confirm() {
    return this.dispatch({ type: "confirm" });
  }

  renewConsent() {
    return this.dispatch({ type: "renewConsent" });
  }

  clearPending() {
    return this.dispatch({ type: "clearPending" });
  }

  canSubmit(): boolean {
    return canSubmitWithCapturedContext(this.state);
  }
}