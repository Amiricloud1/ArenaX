import { xdr, scValToNative } from '@stellar/stellar-sdk';
import { EventSchema } from '../event-parser.service.js';

function parseStruct(data: xdr.ScVal): Record<string, unknown> {
    return scValToNative(data) as Record<string, unknown>;
}

export const idRoleSetV0: EventSchema = { namespace: 'ArenaXId', version: 'v0', eventType: 'ROLE_SET', parse: parseStruct };
export const idRoleSetV1: EventSchema = { namespace: 'ArenaXId', version: 'v1', eventType: 'ROLE_SET', parse: parseStruct };

// Identity stitching events. These are stored separately from product event
// payloads and never carry raw authentication identifiers; only opaque,
// purpose-limited link references are emitted on-chain.
export const idIdentityLinkV0: EventSchema = { namespace: 'ArenaXId', version: 'v0', eventType: 'IDENTITY_LINK', parse: parseStruct };
export const idIdentityLinkV1: EventSchema = { namespace: 'ArenaXId', version: 'v1', eventType: 'IDENTITY_LINK', parse: parseStruct };

export const idIdentityUnlinkV0: EventSchema = { namespace: 'ArenaXId', version: 'v0', eventType: 'IDENTITY_UNLINK', parse: parseStruct };
export const idIdentityUnlinkV1: EventSchema = { namespace: 'ArenaXId', version: 'v1', eventType: 'IDENTITY_UNLINK', parse: parseStruct };

export const idConsentWithdrawalV0: EventSchema = { namespace: 'ArenaXId', version: 'v0', eventType: 'CONSENT_WITHDRAWAL', parse: parseStruct };
export const idConsentWithdrawalV1: EventSchema = { namespace: 'ArenaXId', version: 'v1', eventType: 'CONSENT_WITHDRAWAL', parse: parseStruct };