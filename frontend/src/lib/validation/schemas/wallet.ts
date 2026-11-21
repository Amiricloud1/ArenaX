/**
 * Wallet / transaction request validation schemas.
 */

import { z } from "zod";
import { Field } from "../fields";
import { Validators } from "../validators";
import { defineSchema } from "../schema";

// ─── Withdraw ─────────────────────────────────────────────────────────────────

export const withdrawSchema = defineSchema(
  "withdraw",
  {
    asset: Field.enum(["XLM", "USDC", "ARENAX"] as const, {
      message: "Select a valid asset",
    }),
    amount: Field.positiveAmountString(),
    destination: Validators.stellarAddress(),
    memo: Validators.stellarMemo(),
  },
  { description: "Withdraw assets from wallet", tags: ["wallet", "payment"] },
);

export type WithdrawRequest = z.infer<typeof withdrawSchema.schema>;

// ─── Deposit ──────────────────────────────────────────────────────────────────

export const depositSchema = defineSchema(
  "deposit",
  {
    asset: Field.enum(["XLM", "USDC", "ARENAX"] as const, {
      message: "Select a valid asset",
    }),
    amount: Field.nonNegativeAmountString(),
  },
  { description: "Deposit assets to wallet", tags: ["wallet", "payment"] },
);

export type DepositRequest = z.infer<typeof depositSchema.schema>;

// ─── Staking ──────────────────────────────────────────────────────────────────

export const stakeSchema = defineSchema(
  "stake",
  {
    asset: Field.enum(["ARENAX"] as const, {
      message: "Only ARENAX tokens can be staked",
    }),
    amount: Field.positiveAmountString({ message: "Enter a valid stake amount greater than 0" }),
    durationDays: Field.integer({
      min: 7,
      max: 365,
      minMessage: "Minimum staking duration is 7 days",
      maxMessage: "Maximum staking duration is 365 days",
    }),
  },
  { description: "Stake ARENAX tokens", tags: ["wallet", "staking"] },
);

export type StakeRequest = z.infer<typeof stakeSchema.schema>;

// ─── Wallet context ───────────────────────────────────────────────────────────

/**
 * Captured wallet context: the account and network a flow was started under.
 * Used to detect account/network switches while a transaction flow is open.
 */
export const walletContextSchema = defineSchema(
  "walletContext",
  {
    account: Validators.stellarAddress(),
    network: Field.string({
      min: 1,
      max: 64,
      message: "Network id is required",
    }),
    chainId: Field.string({
      min: 1,
      max: 64,
      message: "Chain id is required",
    }),
  },
  { description: "Wallet account and network context", tags: ["wallet"] },
);

export type WalletContext = z.infer<typeof walletContextSchema.schema>;

// ─── Confirmed transaction ────────────────────────────────────────────────────

/**
 * A transaction submitted for signing. It must remain bound to the account and
 * network captured at confirmation; if the live context has drifted, the
 * submission is rejected unless the user has explicitly renewed consent.
 */
export const confirmedTransactionSchema = defineSchema(
  "confirmedTransaction",
  {
    signingAccount: Validators.stellarAddress(),
    signingNetwork: Field.string({
      min: 1,
      max: 64,
      message: "Signing network is required",
    }),
    signingChainId: Field.string({
      min: 1,
      max: 64,
      message: "Signing chain id is required",
    }),
    capturedContext: walletContextSchema.schema,
    currentContext: walletContextSchema.schema.optional(),
    renewedConsent: Field.boolean().optional().default(false),
  },
  { description: "Transaction bound to a captured wallet context", tags: ["wallet", "payment"] },
).refine(
  (value) => {
    const { capturedContext, signingAccount, signingNetwork, signingChainId } = value;
    return (
      signingAccount === capturedContext.account &&
      signingNetwork === capturedContext.network &&
      signingChainId === capturedContext.chainId
    );
  },
  {
    message: "Signing account and network must match the captured wallet context",
    path: ["signingAccount"],
  },
).refine(
  (value) => {
    const { capturedContext, currentContext, renewedConsent } = value;
    if (!currentContext) return true;
    const matches =
      currentContext.account === capturedContext.account &&
      currentContext.network === capturedContext.network &&
      currentContext.chainId === capturedContext.chainId;
    return matches || renewedConsent === true;
  },
  {
    message: "Wallet account or network changed; renewed consent is required",
    path: ["currentContext"],
  },
);

export type ConfirmedTransaction = z.infer<typeof confirmedTransactionSchema.schema>;
