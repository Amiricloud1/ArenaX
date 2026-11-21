/**
 * ArenaX TanStack Query Hooks — Issue #693
 *
 * Deterministic caching layer built on TanStack Query v5.
 * All query keys are exported as constants to enable precise cache invalidation.
 *
 * Covers:
 * - Tournaments  (list + detail + participants)
 * - Matches      (list + detail + active)
 * - Profile      (current user + public)
 * - Leaderboard
 * - Notifications
 * - Governance
 * - Wallet
 * - Disputes / Audit logs (admin)
 */

"use client";

import {
  useQuery,
  useMutation,
  useInfiniteQuery,
  useQueryClient,
  type UseQueryOptions,
  type UseMutationOptions,
  type InfiniteData,
} from "@tanstack/react-query";
import { apiClient } from "./apiClient";
import type { ApiResponse, PaginatedResponse } from "@/types";
import type { MatchWithPlayers } from "@/types/match";
import type { Tournament } from "@/types/tournament";

// ─── Query Keys (type-safe constant map) ──────────────────────────────────────

export const QK = {
  // Tournaments
  tournaments: {
    all: ["tournaments"] as const,
    list: (params?: Record<string, unknown>) => ["tournaments", "list", params] as const,
    detail: (id: string) => ["tournaments", id] as const,
    participants: (id: string) => ["tournaments", id, "participants"] as const,
  },
  // Matches
  matches: {
    all: ["matches"] as const,
    list: (params?: Record<string, unknown>) => ["matches", "list", params] as const,
    detail: (id: string) => ["matches", id] as const,
    active: ["matches", "active"] as const,
  },
  // Profile / User
  profile: {
    me: ["profile", "me"] as const,
    public: (id: string) => ["profile", "public", id] as const,
  },
  // Leaderboard
  leaderboard: {
    all: ["leaderboard"] as const,
    byPeriod: (period: string) => ["leaderboard", period] as const,
  },
  // Notifications
  notifications: {
    all: ["notifications"] as const,
  },
  // Governance
  governance: {
    all: ["governance"] as const,
    detail: (id: string) => ["governance", id] as const,
  },
  // Wallet
  wallet: {
    balance: ["wallet", "balance"] as const,
    transactions: (params?: Record<string, unknown>) => ["wallet", "transactions", params] as const,
  },
  // Admin
  admin: {
    disputes: (params?: Record<string, unknown>) => ["admin", "disputes", params] as const,
    auditLogs: (params?: Record<string, unknown>) => ["admin", "auditLogs", params] as const,
    kycReviews: (params?: Record<string, unknown>) => ["admin", "kycReviews", params] as const,
  },
  // Dashboard analytics (budgeted, catalogued metrics)
  analytics: {
    all: ["analytics"] as const,
    metric: (metricId: string, params?: Record<string, unknown>) =>
      ["analytics", metricId, params] as const,
  },
} as const;

// ─── Stale-time constants ─────────────────────────────────────────────────────

const STALE = {
  SHORT: 15_000,      // 15 s — live data (active matches, notifications)
  MEDIUM: 60_000,     // 1 min — semi-live (leaderboard, active tournaments)
  LONG: 5 * 60_000,   // 5 min — mostly static (profiles, governance)
  IMMUTABLE: Infinity, // Never re-fetch (e.g. historical records)
} as const;

// ─── Analytics budgets & governed catalog (Issues #253 / #254) ────────────────

/**
 * Budget metadata attached to every dashboard query. Each query must declare:
 * - `metricId`: the governed catalog entry (owner, definition, sources, privacy
 *   class, deprecation state) that the dashboard renders.
 * - `latencyTargetMs`: p95 latency target for the query.
 * - `freshnessTargetMs`: maximum acceptable data age before it is "stale".
 * - `maxRangeMs`: bounded time range the query may request.
 * - `maxPageSize`: pagination cap enforced client-side.
 */
export interface AnalyticsBudget {
  metricId: string;
  latencyTargetMs: number;
  freshnessTargetMs: number;
  maxRangeMs: number;
  maxPageSize: number;
}

/** Server-reported budget status for a dashboard response. */
export interface BudgetStatus {
  /** True when the backend flagged the result as exceeding a budget. */
  exceeded?: boolean;
  /** True when the result is known to be incomplete (truncated / partial). */
  incomplete?: boolean;
  /** Age of the underlying data in ms, as reported by the backend. */
  dataAgeMs?: number;
  /** Server-side query duration in ms, as reported by the backend. */
  queryDurationMs?: number;
  /** Machine-readable reason codes (e.g. "timeout", "range_truncated"). */
  reasons?: string[];
}

/** Normalised stale/incomplete marker surfaced to dashboard components. */
export interface BudgetMarker {
  stale: boolean;
  incomplete: boolean;
  reasons: string[];
  dataAgeMs?: number;
  queryDurationMs?: number;
}

export const ANALYTICS_BUDGETS = {
  walletBalance: {
    metricId: "wallet.balance.current",
    latencyTargetMs: 500,
    freshnessTargetMs: 30_000,
    maxRangeMs: 0, // point-in-time snapshot, no range
    maxPageSize: 1,
  },
  walletTransactions: {
    metricId: "wallet.transactions.volume",
    latencyTargetMs: 1_500,
    freshnessTargetMs: 60_000,
    maxRangeMs: 90 * 24 * 60 * 60_000, // 90 days
    maxPageSize: 100,
  },
  tournamentList: {
    metricId: "tournament.list.active",
    latencyTargetMs: 800,
    freshnessTargetMs: 60_000,
    maxRangeMs: 30 * 24 * 60 * 60_000, // 30 days
    maxPageSize: 50,
  },
  tournamentParticipants: {
    metricId: "tournament.participants.count",
    latencyTargetMs: 800,
    freshnessTargetMs: 30_000,
    maxRangeMs: 0,
    maxPageSize: 500,
  },
  leaderboard: {
    metricId: "retention.leaderboard.weekly",
    latencyTargetMs: 1_000,
    freshnessTargetMs: 60_000,
    maxRangeMs: 7 * 24 * 60 * 60_000, // 7 days
    maxPageSize: 100,
  },
  serviceHealth: {
    metricId: "service.health.audit",
    latencyTargetMs: 2_000,
    freshnessTargetMs: 5 * 60_000,
    maxRangeMs: 30 * 24 * 60 * 60_000, // 30 days
    maxPageSize: 200,
  },
} as const satisfies Record<string, AnalyticsBudget>;

export type AnalyticsBudgetKey = keyof typeof ANALYTICS_BUDGETS;

/** Clamps a requested page size to the budget's cap. */
export function clampPageSize(requested: number | undefined, budget: AnalyticsBudget): number {
  const size = requested ?? budget.maxPageSize;
  return Math.max(1, Math.min(size, budget.maxPageSize));
}

/**
 * Extracts a `BudgetStatus` from a raw API payload. Accepts either a top-level
 * `budget` field or a `meta.budget` field so both envelope styles are covered.
 */
export function extractBudgetStatus(payload: unknown): BudgetStatus | undefined {
  if (!payload || typeof payload !== "object") return undefined;
  const obj = payload as Record<string, unknown>;
  const direct = obj["budget"];
  if (direct && typeof direct === "object") return direct as BudgetStatus;
  const meta = obj["meta"];
  if (meta && typeof meta === "object") {
    const nested = (meta as Record<string, unknown>)["budget"];
    if (nested && typeof nested === "object") return nested as BudgetStatus;
  }
  return undefined;
}

/**
 * Builds the visible stale/incomplete marker for a dashboard query. A result is
 * considered stale when the backend flags an exceeded budget or when the
 * reported data age exceeds the freshness target.
 */
export function buildBudgetMarker(
  budget: AnalyticsBudget,
  status: BudgetStatus | undefined,
): BudgetMarker {
  const reasons = status?.reasons ? [...status.reasons] : [];
  const ageExceeded =
    typeof status?.dataAgeMs === "number" && status.dataAgeMs > budget.freshnessTargetMs;
  const latencyExceeded =
    typeof status?.queryDurationMs === "number" &&
    status.queryDurationMs > budget.latencyTargetMs;
  if (ageExceeded && !reasons.includes("freshness_exceeded")) reasons.push("freshness_exceeded");
  if (latencyExceeded && !reasons.includes("latency_exceeded")) reasons.push("latency_exceeded");
  const stale = Boolean(status?.exceeded) || ageExceeded || latencyExceeded;
  const incomplete = Boolean(status?.incomplete) || reasons.includes("range_truncated");
  return {
    stale,
    incomplete,
    reasons,
    dataAgeMs: status?.dataAgeMs,
    queryDurationMs: status?.queryDurationMs,
  };
}

/** Result shape returned by budgeted dashboard hooks. */
export interface BudgetedResult<T> {
  data: T;
  budget: AnalyticsBudget;
  marker: BudgetMarker;
}

// ─── Cache invalidation helpers ───────────────────────────────────────────────

export function useCacheInvalidation() {
  const qc = useQueryClient();

  return {
    invalidateTournaments: () => qc.invalidateQueries({ queryKey: QK.tournaments.all }),
    invalidateTournament: (id: string) => qc.invalidateQueries({ queryKey: QK.tournaments.detail(id) }),
    invalidateMatches: () => qc.invalidateQueries({ queryKey: QK.matches.all }),
    invalidateMatch: (id: string) => qc.invalidateQueries({ queryKey: QK.matches.detail(id) }),
    invalidateProfile: () => qc.invalidateQueries({ queryKey: QK.profile.me }),
    invalidateNotifications: () => qc.invalidateQueries({ queryKey: QK.notifications.all }),
    invalidateLeaderboard: () => qc.invalidateQueries({ queryKey: QK.leaderboard.all }),
    invalidateWallet: () => qc.invalidateQueries({ queryKey: QK.wallet.balance }),
    invalidateGovernance: () => qc.invalidateQueries({ queryKey: QK.governance.all }),
    invalidateAll: () => qc.invalidateQueries(),
  };
}

// ─── Tournaments ──────────────────────────────────────────────────────────────

export type TournamentParams = {
  page?: number;
  limit?: number;
  status?: string;
  gameType?: string;
  search?: string;
  pageStatus?: string;
  tournamentType?: string;
  visibility?: string;
  minEntryFee?: number;
  maxEntryFee?: number;
  minPrizePool?: number;
  maxPrizePool?: number;
  sortBy?: string;
  sortOrder?: "asc" | "desc";
};

/** Builds a flat query-string map from `TournamentParams`, skipping undefined. */
function tournamentQueryParams(params?: TournamentParams): Record<string, string> {
  const p: Record<string, string> = {};
  if (params?.status)         p["status"]         = params.status;
  if (params?.gameType)       p["gameType"]       = params.gameType;
  if (params?.search)         p["search"]         = params.search;
  if (params?.pageStatus)     p["pageStatus"]     = params.pageStatus;
  if (params?.tournamentType) p["tournamentType"] = params.tournamentType;
  if (params?.visibility)     p["visibility"]     = params.visibility;
  if (params?.sortBy)         p["sortBy"]         = params.sortBy;
  if (params?.sortOrder)      p["sortOrder"]      = params.sortOrder;
  if (params?.minEntryFee !== undefined)  p["minEntryFee"]  = String(params.minEntryFee);
  if (params?.maxEntryFee !== undefined)  p["maxEntryFee"]  = String(params.maxEntryFee);
  if (params?.minPrizePool !== undefined) p["minPrizePool"] = String(params.minPrizePool);
  if (params?.maxPrizePool !== undefined) p["maxPrizePool"] = String(params.maxPrizePool);
  return p;
}

export function useTournaments(
  params?: TournamentParams,
  options?: Partial<UseQueryOptions<PaginatedResponse<Tournament>>>,
) {
  return useQuery<PaginatedResponse<Tournament>>({
    queryKey: QK.tournaments.list(params),
    queryFn: async () => {
      const p: Record<string, string> = {};
      if (params?.page != null) p["page"] = String(params.page);
      if (params?.limit != null) p["limit"] = String(params.limit);
      if (params?.status) p["status"] = params.status;
      if (params?.gameType) p["gameType"] = params.gameType;
      if (params?.search) p["search"] = params.search;
      return apiClient.getPaginated<Tournament>("/tournaments", { params: p });
    },
    staleTime: STALE.MEDIUM,
    ...options,
  });
}

export function useTournament(id: string, options?: Partial<UseQueryOptions<Tournament>>) {
  return useQuery<Tournament>({
    queryKey: QK.tournaments.detail(id),
    queryFn: () => apiClient.getEnveloped<Tournament>(`/tournaments/${id}`),
    staleTime: STALE.MEDIUM,
    enabled: Boolean(id),
    ...options,
  });
}

export function useTournamentParticipants(id: string) {
  return useQuery({
    queryKey: QK.tournaments.participants(id),
    queryFn: () => apiClient.getEnveloped(`/tournaments/${id}/participants`),
    staleTime: STALE.SHORT,
    enabled: Boolean(id),
  });
}

export function useJoinTournament(
  options?: UseMutationOptions<unknown, Error, string>,
) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiClient.post(`/tournaments/${id}/register`),
    onSuccess: (_data, id) => {
      qc.invalidateQueries({ queryKey: QK.tournaments.detail(id) });
      qc.invalidateQueries({ queryKey: QK.tournaments.all });
    },
    ...options,
  });
}

export function useCreateTournament(
  options?: UseMutationOptions<Tournament, Error, Partial<Tournament>>,
) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: Partial<Tournament>) => apiClient.post<Tournament>("/tournaments", data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK.tournaments.all });
    },
    ...options,
  });
}

// ─── Infinite tournaments list ────────────────────────────────────────────────

export function useInfiniteTournaments(params?: Omit<TournamentParams, "page">) {
  return useInfiniteQuery<PaginatedResponse<Tournament>, Error, InfiniteData<PaginatedResponse<Tournament>>, ReturnType<typeof QK.tournaments.list>, number>({
    queryKey: QK.tournaments.list(params),
    queryFn: ({ pageParam = 1 }) => {
      const p = tournamentQueryParams(params);
      p["page"] = String(pageParam);
      p["limit"] = String(params?.limit ?? 20);
      return apiClient.getPaginated<Tournament>("/tournaments", { params: p });
    },
    initialPageParam: 1,
    getNextPageParam: (last) => {
      const page = last.page ?? 1;
      const limit = last.limit || 20;
      const total = typeof last.total === "number" ? last.total : 0;
      // Prefer the server's own totalPages when present; fall back to a
      // total/limit computation or a full-page heuristic so the list keeps
      // loading even if the backend omits paging fields.
      const totalPages =
        typeof last.totalPages === "number"
          ? last.totalPages
          : total > 0
            ? Math.ceil(total / limit)
            : Math.max(page, 1);
      const hasMore =
        page < totalPages || (Array.isArray(last.data) && last.data.length >= limit);
      return hasMore ? page + 1 : undefined;
    },
    staleTime: STALE.MEDIUM,
  });
}

// ─── Matches ──────────────────────────────────────────────────────────────────

export type MatchParams = {
  page?: number;
  limit?: number;
  status?: string;
  mine?: boolean;
};

export function useMatches(params?: MatchParams, options?: Partial<UseQueryOptions<PaginatedResponse<MatchWithPlayers>>>) {
  return useQuery<PaginatedResponse<MatchWithPlayers>>({
    queryKey: QK.matches.list(params),
    queryFn: () => {
      const p: Record<string, string> = {};
      if (params?.page != null) p["page"] = String(params.page);
      if (params?.limit != null) p["limit"] = String(params.limit);
      if (params?.status) p["status"] = params.status;
      if (params?.mine) p["mine"] = "true";
      return apiClient.getPaginated<MatchWithPlayers>("/matches", { params: p });
    },
    staleTime: STALE.SHORT,
    ...options,
  });
}

export function useMatch(id: string, options?: Partial<UseQueryOptions<MatchWithPlayers>>) {
  return useQuery<MatchWithPlayers>({
    queryKey: QK.matches.detail(id),
    queryFn: () => apiClient.getEnveloped<MatchWithPlayers>(`/matches/${id}`),
    staleTime: STALE.SHORT,
    enabled: Boolean(id),
    ...options,
  });
}

export function useActiveMatches() {
  return useQuery<MatchWithPlayers[]>({
    queryKey: QK.matches.active,
    queryFn: async () => {
      const res = await apiClient.get<ApiResponse<MatchWithPlayers[]> | MatchWithPlayers[]>(
        "/matches",
        { params: { status: "in_progress", mine: "true" } },
      );
      if (Array.isArray(res)) return res;
      return (res as ApiResponse<MatchWithPlayers[]>).data ?? [];
    },
    staleTime: STALE.SHORT,
    refetchInterval: STALE.SHORT,
  });
}

export function useReportMatchScore() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, result }: { id: string; result: unknown }) =>
      apiClient.post(`/matches/${id}/report`, result),
    onSuccess: (_data, { id }) => {
      qc.invalidateQueries({ queryKey: QK.matches.detail(id) });
      qc.invalidateQueries({ queryKey: QK.matches.active });
    },
  });
}

// ─── Profile ──────────────────────────────────────────────────────────────────

export interface UserProfile {
  id: string;
  username: string;
  email: string | null;
  is_verified: boolean;
  created_at: string;
  elo?: number;
  avatar?: string;
  bio?: string;
}

export function useCurrentProfile(options?: Partial<UseQueryOptions<UserProfile>>) {
  return useQuery<UserProfile>({
    queryKey: QK.profile.me,
    queryFn: async () => {
      const res = await apiClient.get<ApiResponse<UserProfile> | UserProfile>("/users/me");
      if ("data" in (res as object) && (res as ApiResponse<UserProfile>).data) {
        return (res as ApiResponse<UserProfile>).data;
      }
      return res as UserProfile;
    },
    staleTime: STALE.LONG,
    ...options,
  });
}

export function usePublicProfile(id: string, options?: Partial<UseQueryOptions<UserProfile>>) {
  return useQuery<UserProfile>({
    queryKey: QK.profile.public(id),
    queryFn: () => apiClient.getEnveloped<UserProfile>(`/users/${id}`),
    staleTime: STALE.LONG,
    enabled: Boolean(id),
    ...options,
  });
}

export function useUpdateProfile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: Partial<UserProfile>) => apiClient.patch("/users/me", data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK.profile.me });
    },
  });
}

// ─── Leaderboard ──────────────────────────────────────────────────────────────

export function useLeaderboard(period = "weekly") {
  return useQuery({
    queryKey: QK.leaderboard.byPeriod(period),
    queryFn: () => apiClient.get(`/leaderboard`, { params: { period } }),
    staleTime: STALE.MEDIUM,
    refetchInterval: STALE.MEDIUM,
  });
}

/**
 * Budgeted leaderboard query (retention metric). Surfaces a stale/incomplete
 * marker when the backend reports an exceeded freshness or latency budget.
 */
export function useBudgetedLeaderboard(period = "weekly") {
  const budget = ANALYTICS_BUDGETS.leaderboard;
  return useQuery<BudgetedResult<unknown>>({
    queryKey: QK.analytics.metric(budget.metricId, { period }),
    queryFn: async () => {
      const res = await apiClient.get(`/leaderboard`, { params: { period } });
      const status = extractBudgetStatus(res);
      return { data: res, budget, marker: buildBudgetMarker(budget, status) };
    },
    staleTime: STALE.MEDIUM,
    refetchInterval: STALE.MEDIUM,
  });
}

// ─── Notifications ────────────────────────────────────────────────────────────

export interface ApiNotification {
  id: string;
  type: string;
  title: string;
  message: string;
  link?: string;
  linkLabel?: string;
  read: boolean;
  createdAt: string;
}

export function useNotifications(options?: Partial<UseQueryOptions<ApiNotification[]>>) {
  return useQuery<ApiNotification[]>({
    queryKey: QK.notifications.all,
    queryFn: async () => {
      try {
        const res = await apiClient.get<ApiResponse<ApiNotification[]> | ApiNotification[]>("/notifications");
        if (Array.isArray(res)) return res;
        return (res as ApiResponse<ApiNotification[]>).data ?? [];
      } catch {
        return [];
      }
    },
    staleTime: STALE.SHORT,
    refetchInterval: STALE.SHORT,
    ...options,
  });
}

export function useMarkNotificationRead() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiClient.patch(`/notifications/${id}/read`),
    onSuccess: () => qc.invalidateQueries({ queryKey: QK.notifications.all }),
  });
}

export function useMarkAllNotificationsRead() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiClient.patch("/notifications/read-all"),
    onSuccess: () => qc.invalidateQueries({ queryKey: QK.notifications.all }),
  });
}

// ─── Governance ───────────────────────────────────────────────────────────────

export function useProposals() {
  return useQuery({
    queryKey: QK.governance.all,
    queryFn: async () => {
      try {
        const res = await apiClient.get<ApiResponse<unknown[]> | unknown[]>("/governance");
        if (Array.isArray(res)) return res;
        return (res as ApiResponse<unknown[]>).data ?? [];
      } catch {
        return [];
      }
    },
    staleTime: STALE.LONG,
  });
}

export function useProposal(id: string) {
  return useQuery({
    queryKey: QK.governance.detail(id),
    queryFn: () => apiClient.get(`/governance/${id}`),
    staleTime: STALE.LONG,
    enabled: Boolean(id),
  });
}

export function useVoteOnProposal() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, signature }: { id: string; signature?: string }) =>
      apiClient.post(`/governance/${id}/vote`, { signature }),
    onSuccess: (_data, { id }) => {
      qc.invalidateQueries({ queryKey: QK.governance.detail(id) });
      qc.invalidateQueries({ queryKey: QK.governance.all });
    },
  });
}

// ─── Admin ────────────────────────────────────────────────────────────────────

export function useDisputes(params?: Record<string, unknown>) {
  return useQuery({
    queryKey: QK.admin.disputes(params),
    queryFn: () => {
      const p: Record<string, string> = {};
      if (params) Object.entries(params).forEach(([k, v]) => { if (v != null) p[k] = String(v); });
      return apiClient.get("/admin/disputes", { params: p });
    },
    staleTime: STALE.SHORT,
  });
}

export function useResolveDispute() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, data }: { id: string; data: { status: string; resolution: string; winnerOverrideId?: string } }) =>
      apiClient.post(`/admin/disputes/${id}/resolve`, data),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "disputes"] }),
  });
}

export function useAuditLogs(params?: Record<string, unknown>) {
  return useQuery({
    queryKey: QK.admin.auditLogs(params),
    queryFn: () => {
      const p: Record<string, string> = {};
      if (params) Object.entries(params).forEach(([k, v]) => { if (v != null) p[k] = String(v); });
      return apiClient.get("/admin/audit-logs", { params: p });
    },
    staleTime: STALE.MEDIUM,
  });
}

/**
 * Budgeted service-health audit query. Enforces the pagination cap and bounded
 * time range, and surfaces a stale/incomplete marker on exceeded budgets.
 */
export function useBudgetedServiceHealth(params?: { page?: number; limit?: number; from?: string; to?: string }) {
  const budget = ANALYTICS_BUDGETS.serviceHealth;
  const limit = clampPageSize(params?.limit, budget);
  return useQuery<BudgetedResult<unknown>>({
    queryKey: QK.analytics.metric(budget.metricId, { ...params, limit }),
    queryFn: async () => {
      const p: Record<string, string> = { limit: String(limit) };
      if (params?.page != null) p["page"] = String(params.page);
      if (params?.from) p["from"] = params.from;
      if (params?.to) p["to"] = params.to;
      const res = await apiClient.get("/admin/audit-logs", { params: p });
      const status = extractBudgetStatus(res);
      return { data: res, budget, marker: buildBudgetMarker(budget, status) };
    },
    staleTime: STALE.MEDIUM,
  });
}

// ─── Budgeted wallet analytics ────────────────────────────────────────────────

/**
 * Budgeted wallet transaction-volume query. Enforces the pagination cap and
 * bounded time range, and surfaces a stale/incomplete marker on exceeded
 * budgets. Financial state remains server-authoritative — this hook only reads.
 */
export function useBudgetedWalletTransactions(params?: { page?: number; limit?: number; from?: string; to?: string }) {
  const budget = ANALYTICS_BUDGETS.walletTransactions;
  const limit = clampPageSize(params?.limit, budget);
  return useQuery<BudgetedResult<unknown>>({
    queryKey: QK.analytics.metric(budget.metricId, { ...params, limit }),
    queryFn: async () => {
      const p: Record<string, string> = { limit: String(limit) };
      if (params?.page != null) p["page"] = String(params.page);
      if (params?.from) p["from"] = params.from;
      if (params?.to) p["to"] = params.to;
      const res = await apiClient.get("/wallet/transactions", { params: p });
      const status = extractBudgetStatus(res);
      return { data: res, budget, marker: buildBudgetMarker(budget, status) };
    },
    staleTime: STALE.MEDIUM,
  });
}

/**
 * Budgeted wallet balance query. Point-in-time snapshot; no range or pagination.
 * Surfaces a stale marker when the backend reports exceeded freshness budgets.
 */
export function useBudgetedWalletBalance() {
  const budget = ANALYTICS_BUDGETS.walletBalance;
  return useQuery<BudgetedResult<unknown>>({
    queryKey: QK.analytics.metric(budget.metricId),
    queryFn: async () => {
      const res = await apiClient.get("/wallet/balance");
      const status = extractBudgetStatus(res);
      return { data: res, budget, marker: buildBudgetMarker(budget, status) };
    },
    staleTime: STALE.SHORT,
  });
}
