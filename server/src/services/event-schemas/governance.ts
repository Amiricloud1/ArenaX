import { xdr, scValToNative } from '@stellar/stellar-sdk';
import { EventSchema } from '../event-parser.service.js';
import {
    AnalyticsEventContract,
    registerAnalyticsEvent,
} from '../analytics-event-registry.service.js';

function parseStruct(data: xdr.ScVal): Record<string, unknown> {
    return scValToNative(data) as Record<string, unknown>;
}

/**
 * Governance-domain analytics event contracts (#249).
 *
 * These describe the off-chain analytics events emitted by frontend and
 * server producers when users interact with governance. They are versioned
 * independently of the on-chain `ArenaXGov` event schemas below, and are
 * registered into the shared cross-client analytics registry so producer and
 * consumer validation stays in sync.
 *
 * Prohibited attributes are enforced by the registry: any payload carrying a
 * prohibited key is rejected before it reaches a consumer.
 */
const GOVERNANCE_OWNER = 'governance';
const GOVERNANCE_PRODUCERS = ['frontend', 'server'] as const;

const GOVERNANCE_PROHIBITED = [
    'privateKey',
    'secretKey',
    'seed',
    'mnemonic',
    'signature',
    'signedXdr',
    'rawTransaction',
    'walletAddress',
    'email',
    'ipAddress',
] as const;

export const governanceAnalyticsEvents: AnalyticsEventContract[] = [
    {
        name: 'governance.proposal.viewed',
        owner: GOVERNANCE_OWNER,
        purpose: 'Measure proposal engagement and detail-page reach.',
        version: 1,
        producers: [...GOVERNANCE_PRODUCERS],
        requiredFields: {
            proposalId: 'string',
            schemaVersion: 'number',
        },
        optionalFields: {
            source: 'string',
            referrer: 'string',
        },
        prohibitedFields: [...GOVERNANCE_PROHIBITED],
    },
    {
        name: 'governance.proposal.created',
        owner: GOVERNANCE_OWNER,
        purpose: 'Track proposal creation volume and authorship funnel.',
        version: 1,
        producers: [...GOVERNANCE_PRODUCERS],
        requiredFields: {
            proposalId: 'string',
            schemaVersion: 'number',
        },
        optionalFields: {
            category: 'string',
            quorum: 'number',
        },
        prohibitedFields: [...GOVERNANCE_PROHIBITED],
    },
    {
        name: 'governance.vote.cast',
        owner: GOVERNANCE_OWNER,
        purpose: 'Measure voting participation and outcome distribution.',
        version: 1,
        producers: [...GOVERNANCE_PRODUCERS],
        requiredFields: {
            proposalId: 'string',
            choice: 'string',
            schemaVersion: 'number',
        },
        optionalFields: {
            weight: 'number',
            viaDelegate: 'boolean',
        },
        prohibitedFields: [...GOVERNANCE_PROHIBITED],
    },
    {
        name: 'governance.vote.delegated',
        owner: GOVERNANCE_OWNER,
        purpose: 'Track delegation usage without exposing voter identity.',
        version: 1,
        producers: [...GOVERNANCE_PRODUCERS],
        requiredFields: {
            proposalId: 'string',
            schemaVersion: 'number',
        },
        optionalFields: {
            delegateCount: 'number',
        },
        prohibitedFields: [...GOVERNANCE_PROHIBITED],
    },
    {
        name: 'governance.proposal.executed',
        owner: GOVERNANCE_OWNER,
        purpose: 'Confirm execution outcomes for operational dashboards.',
        version: 1,
        producers: ['server'],
        requiredFields: {
            proposalId: 'string',
            schemaVersion: 'number',
        },
        optionalFields: {
            txHash: 'string',
        },
        prohibitedFields: [...GOVERNANCE_PROHIBITED],
    },
];

for (const contract of governanceAnalyticsEvents) {
    registerAnalyticsEvent(contract);
}

export const govInitV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'INIT', parse: parseStruct };
export const govInitV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'INIT', parse: parseStruct };
export const govProposedV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'PROPOSED', parse: parseStruct };
export const govProposedV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'PROPOSED', parse: parseStruct };
export const govApprovedV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'APPROVED', parse: parseStruct };
export const govApprovedV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'APPROVED', parse: parseStruct };
export const govRevokedV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'REVOKED', parse: parseStruct };
export const govRevokedV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'REVOKED', parse: parseStruct };
export const govExecutedV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'EXECUTED', parse: parseStruct };
export const govExecutedV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'EXECUTED', parse: parseStruct };
export const govCancelledV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'CANCELLED', parse: parseStruct };
export const govCancelledV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'CANCELLED', parse: parseStruct };
export const govSignerAddV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'SIGNER_ADD', parse: parseStruct };
export const govSignerAddV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'SIGNER_ADD', parse: parseStruct };
export const govSignerRemV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'SIGNER_REM', parse: parseStruct };
export const govSignerRemV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'SIGNER_REM', parse: parseStruct };
export const govThreshUpdV0: EventSchema = { namespace: 'ArenaXGov', version: 'v0', eventType: 'THRESH_UPD', parse: parseStruct };
export const govThreshUpdV1: EventSchema = { namespace: 'ArenaXGov', version: 'v1', eventType: 'THRESH_UPD', parse: parseStruct };
