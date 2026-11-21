import { EventProducer } from './event-producer';
import {
  TOPICS,
  MatchEndedPayload,
  UserRegisteredPayload,
  AchievementUnlockedPayload,
  WalletCreditedPayload,
} from './domain-events';
import {
  assertValidEventPayload,
  getEventSchema,
  EventSchemaName,
} from '../../../../shared/analytics/event-schema-registry';
import { isAnalyticsConsentGranted } from '../analytics/consent-store';

// ─── Shared helpers ───────────────────────────────────────────────────────────

/**
 * Validates a payload against the shared event schema registry before it is
 * published. Throws a descriptive error on schema violations so producers fail
 * fast instead of emitting malformed events into the queue.
 */
function validatePayload<T>(schemaName: EventSchemaName, payload: T): void {
  assertValidEventPayload(schemaName, payload);
}

/**
 * Authoritative consent gate for the queued-delivery path. Non-essential
 * events are suppressed when the subject has withdrawn consent (or when no
 * consent record exists). Essential events bypass this check.
 */
async function isDeliveryAllowed(
  schemaName: EventSchemaName,
  subjectId: string | undefined,
): Promise<boolean> {
  const schema = getEventSchema(schemaName);
  if (schema.essential) return true;
  if (!subjectId) return false;
  return isAnalyticsConsentGranted(subjectId);
}

// ─── MatchProducer ────────────────────────────────────────────────────────────

export class MatchProducer extends EventProducer<MatchEndedPayload> {
  protected readonly topic = TOPICS.MATCH_EVENTS;
  protected readonly eventType = 'match.ended';
  protected readonly version = getEventSchema('match.ended').version;

  async publishMatchEnded(payload: MatchEndedPayload, traceId: string): Promise<void> {
    validatePayload('match.ended', payload);
    if (!(await isDeliveryAllowed('match.ended', payload.matchId))) return;
    // Partition by matchId so all events for the same match land on the same partition
    await this.publish(payload, traceId, payload.matchId);
  }
}

// ─── UserProducer ─────────────────────────────────────────────────────────────

export class UserProducer extends EventProducer<UserRegisteredPayload> {
  protected readonly topic = TOPICS.USER_EVENTS;
  protected readonly eventType = 'user.registered';
  protected readonly version = getEventSchema('user.registered').version;

  async publishUserRegistered(payload: UserRegisteredPayload, traceId: string): Promise<void> {
    validatePayload('user.registered', payload);
    if (!(await isDeliveryAllowed('user.registered', payload.userId))) return;
    await this.publish(payload, traceId, payload.userId);
  }
}

// ─── AchievementProducer ──────────────────────────────────────────────────────

export class AchievementProducer extends EventProducer<AchievementUnlockedPayload> {
  protected readonly topic = TOPICS.ACHIEVEMENT_EVENTS;
  protected readonly eventType = 'achievement.unlocked';
  protected readonly version = getEventSchema('achievement.unlocked').version;

  async publishAchievementUnlocked(payload: AchievementUnlockedPayload, traceId: string): Promise<void> {
    validatePayload('achievement.unlocked', payload);
    if (!(await isDeliveryAllowed('achievement.unlocked', payload.userId))) return;
    await this.publish(payload, traceId, payload.userId);
  }
}

// ─── WalletProducer ───────────────────────────────────────────────────────────

export class WalletProducer extends EventProducer<WalletCreditedPayload> {
  protected readonly topic = TOPICS.WALLET_EVENTS;
  protected readonly eventType = 'wallet.credited';
  protected readonly version = getEventSchema('wallet.credited').version;

  async publishWalletCredited(payload: WalletCreditedPayload, traceId: string): Promise<void> {
    validatePayload('wallet.credited', payload);
    if (!(await isDeliveryAllowed('wallet.credited', payload.userId))) return;
    await this.publish(payload, traceId, payload.userId);
  }
}

// ─── Singleton instances ──────────────────────────────────────────────────────

export const matchProducer = new MatchProducer();
export const userProducer = new UserProducer();
export const achievementProducer = new AchievementProducer();
export const walletProducer = new WalletProducer();
