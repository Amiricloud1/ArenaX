import { xdr, scValToNative } from '@stellar/stellar-sdk';
import { EventSchema } from '../event-parser.service.js';

function parseStruct(data: xdr.ScVal): Record<string, unknown> {
    return scValToNative(data) as Record<string, unknown>;
}

/**
 * Cross-client analytics event contract.
 *
 * Frontend and server producers must both validate against these definitions so
 * that an event with the same name always carries the same meaning and shape.
 */
export interface AnalyticsEventContract {
    /** Stable event name shared across clients, e.g. `arena.match.completed`. */
    name: string;
    /** Team or service that owns the event definition and review. */
    owner: string;
    /** Why the event exists and what decisions it supports. */
    purpose: string;
    /** Semantic version of the contract. Breaking changes bump the major. */
    schemaVersion: string;
    /** Fields that must be present and non-null. */
    requiredFields: readonly string[];
    /** Fields that may be present. */
    optionalFields: readonly string[];
    /** Attributes that must never be attached (PII, secrets, financial state). */
    prohibitedAttributes: readonly string[];
    /** Migration plan required when a breaking change is introduced. */
    migrationPlan?: string;
}

export interface AnalyticsValidationResult {
    valid: boolean;
    errors: string[];
}

const SENSITIVE_PATTERNS: readonly RegExp[] = [
    /pass(word|phrase)?/i,
    /secret/i,
    /private[_-]?key/i,
    /seed/i,
    /mnemonic/i,
    /token/i,
    /api[_-]?key/i,
    /authorization/i,
    /cookie/i,
    /session/i,
    /email/i,
    /phone/i,
    /ssn/i,
    /credit[_-]?card/i,
    /card[_-]?number/i,
    /balance/i,
    /amount/i,
    /wallet[_-]?address/i,
    /ip[_-]?address/i,
];

function isSensitiveAttribute(key: string): boolean {
    return SENSITIVE_PATTERNS.some((pattern) => pattern.test(key));
}

export const analyticsEventContracts: readonly AnalyticsEventContract[] = [
    {
        name: 'arena.match.completed',
        owner: 'arena-platform',
        purpose: 'Track match completion for engagement and retention analytics.',
        schemaVersion: '1.0.0',
        requiredFields: ['matchId', 'outcome', 'completedAt'],
        optionalFields: ['durationMs', 'playerCount'],
        prohibitedAttributes: ['walletAddress', 'balance', 'amount', 'email'],
    },
    {
        name: 'arena.match.started',
        owner: 'arena-platform',
        purpose: 'Measure match start funnel conversion.',
        schemaVersion: '1.0.0',
        requiredFields: ['matchId', 'startedAt'],
        optionalFields: ['playerCount', 'mode'],
        prohibitedAttributes: ['walletAddress', 'balance', 'amount', 'email'],
    },
    {
        name: 'arena.wallet.connected',
        owner: 'arena-platform',
        purpose: 'Measure wallet connection funnel without storing identifiers.',
        schemaVersion: '1.0.0',
        requiredFields: ['connectedAt'],
        optionalFields: ['walletType', 'chainId'],
        prohibitedAttributes: ['walletAddress', 'privateKey', 'seed', 'mnemonic'],
    },
    {
        name: 'arena.payout.requested',
        owner: 'arena-finance',
        purpose: 'Observe payout request volume; financial state stays server-authoritative.',
        schemaVersion: '1.0.0',
        requiredFields: ['payoutId', 'requestedAt'],
        optionalFields: ['status'],
        prohibitedAttributes: ['amount', 'balance', 'walletAddress', 'cardNumber'],
    },
    {
        name: 'arena.error.client',
        owner: 'arena-platform',
        purpose: 'Capture client-side failure rates for reliability monitoring.',
        schemaVersion: '1.0.0',
        requiredFields: ['errorCode', 'occurredAt'],
        optionalFields: ['route', 'component'],
        prohibitedAttributes: ['stack', 'token', 'cookie', 'session', 'email'],
    },
];

const analyticsContractIndex = new Map<string, AnalyticsEventContract>(
    analyticsEventContracts.map((contract) => [contract.name, contract]),
);

export function getAnalyticsEventContract(name: string): AnalyticsEventContract | undefined {
    return analyticsContractIndex.get(name);
}

/**
 * Validate an analytics event payload against the shared contract registry.
 * Used by both frontend and server producers.
 */
export function validateAnalyticsEvent(
    name: string,
    payload: Record<string, unknown>,
): AnalyticsValidationResult {
    const errors: string[] = [];
    const contract = getAnalyticsEventContract(name);

    if (!contract) {
        return { valid: false, errors: [`Unknown analytics event: ${name}`] };
    }

    if (payload === null || typeof payload !== 'object' || Array.isArray(payload)) {
        return { valid: false, errors: ['Payload must be a plain object'] };
    }

    for (const field of contract.requiredFields) {
        const value = payload[field];
        if (value === undefined || value === null) {
            errors.push(`Missing required field: ${field}`);
        }
    }

    const allowed = new Set<string>([...contract.requiredFields, ...contract.optionalFields]);
    for (const key of Object.keys(payload)) {
        if (!allowed.has(key)) {
            errors.push(`Unexpected field: ${key}`);
        }
        if (isSensitiveAttribute(key) || contract.prohibitedAttributes.includes(key)) {
            errors.push(`Prohibited attribute present: ${key}`);
        }
    }

    return { valid: errors.length === 0, errors };
}

// ArenaXReg events
export const regInitV0: EventSchema = { namespace: 'ArenaXReg', version: 'v0', eventType: 'INIT', parse: parseStruct };
export const regInitV1: EventSchema = { namespace: 'ArenaXReg', version: 'v1', eventType: 'INIT', parse: parseStruct };
export const regRegisterV0: EventSchema = { namespace: 'ArenaXReg', version: 'v0', eventType: 'REGISTER', parse: parseStruct };
export const regRegisterV1: EventSchema = { namespace: 'ArenaXReg', version: 'v1', eventType: 'REGISTER', parse: parseStruct };
export const regUpdateV0: EventSchema = { namespace: 'ArenaXReg', version: 'v0', eventType: 'UPDATE', parse: parseStruct };
export const regUpdateV1: EventSchema = { namespace: 'ArenaXReg', version: 'v1', eventType: 'UPDATE', parse: parseStruct };

// ArenaXCReg (contract-registry) events
export const cregInitV0: EventSchema = { namespace: 'ArenaXCReg', version: 'v0', eventType: 'INIT', parse: parseStruct };
export const cregInitV1: EventSchema = { namespace: 'ArenaXCReg', version: 'v1', eventType: 'INIT', parse: parseStruct };
export const cregRegisteredV0: EventSchema = { namespace: 'ArenaXCReg', version: 'v0', eventType: 'REGISTERED', parse: parseStruct };
export const cregRegisteredV1: EventSchema = { namespace: 'ArenaXCReg', version: 'v1', eventType: 'REGISTERED', parse: parseStruct };
export const cregUpdatedV0: EventSchema = { namespace: 'ArenaXCReg', version: 'v0', eventType: 'UPDATED', parse: parseStruct };
export const cregUpdatedV1: EventSchema = { namespace: 'ArenaXCReg', version: 'v1', eventType: 'UPDATED', parse: parseStruct };
export const cregRemovedV0: EventSchema = { namespace: 'ArenaXCReg', version: 'v0', eventType: 'REMOVED', parse: parseStruct };
export const cregRemovedV1: EventSchema = { namespace: 'ArenaXCReg', version: 'v1', eventType: 'REMOVED', parse: parseStruct };
export const cregPausedV0: EventSchema = { namespace: 'ArenaXCReg', version: 'v0', eventType: 'PAUSED', parse: parseStruct };
export const cregPausedV1: EventSchema = { namespace: 'ArenaXCReg', version: 'v1', eventType: 'PAUSED', parse: parseStruct };
