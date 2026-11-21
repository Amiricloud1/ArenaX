export type {
  TxDirection,
  TxHistoryItem,
  TxKind,
  TxMeta,
  TxPhase,
  TxStatus,
} from "@/lib/wallet/types";

/** Wallet transaction entry (deposit / withdraw) shown in history UIs. */
export type { TxHistoryItem as Transaction } from "@/lib/wallet/types";

/**
 * Captured wallet session context. Used to associate pending quotes,
 * unsigned previews, and confirmed transactions with the account and
 * network that were active when the flow started.
 */
export interface WalletSessionContext {
  /** Connected account address (normalized to lowercase). */
  account: string;
  /** Network identifier (e.g. "mainnet", "testnet"). */
  network: string;
  /** Numeric chain id associated with the network. */
  chainId: number;
}

/** Explicit wallet session state transitions observed by the client. */
export type WalletSessionEvent =
  | { type: "account_changed"; previous: WalletSessionContext; current: WalletSessionContext }
  | { type: "network_changed"; previous: WalletSessionContext; current: WalletSessionContext }
  | { type: "disconnected"; previous: WalletSessionContext }
  | { type: "reconnected"; current: WalletSessionContext };

/** A pending quote captured against a specific wallet session context. */
export interface PendingQuote {
  /** Unique identifier for the quote. */
  id: string;
  /** Wallet session context captured when the quote was created. */
  context: WalletSessionContext;
  /** Timestamp (ms since epoch) when the quote was created. */
  createdAt: number;
  /** Optional expiry timestamp (ms since epoch). */
  expiresAt?: number;
}

/** An unsigned transaction preview captured against a wallet session context. */
export interface UnsignedPreview {
  /** Unique identifier for the preview. */
  id: string;
  /** Wallet session context captured when the preview was created. */
  context: WalletSessionContext;
  /** Timestamp (ms since epoch) when the preview was created. */
  createdAt: number;
}

/**
 * A submitted transaction stamped with the signing account and network
 * captured at confirmation time. Mismatches against the live wallet
 * session require renewed consent before proceeding.
 */
export interface ConfirmedTransaction {
  /** Unique identifier for the confirmed transaction. */
  id: string;
  /** Account that signed (or is expected to sign) the transaction. */
  signingAccount: string;
  /** Network the transaction was confirmed against. */
  signingNetwork: string;
  /** Chain id the transaction was confirmed against. */
  signingChainId: number;
  /** Timestamp (ms since epoch) when the transaction was confirmed. */
  confirmedAt: number;
}

/**
 * Returns true when the captured wallet session context no longer matches
 * the current wallet session context. A mismatch means pending quotes and
 * unsigned previews must be invalidated and renewed consent is required.
 */
export function isContextMismatch(
  captured: WalletSessionContext | null | undefined,
  current: WalletSessionContext | null | undefined,
): boolean {
  if (!captured || !current) {
    return true;
  }
  return (
    captured.account.toLowerCase() !== current.account.toLowerCase() ||
    captured.network !== current.network ||
    captured.chainId !== current.chainId
  );
}