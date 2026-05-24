// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number
  available: number
  enabledCount: number
  schedulableCount: number
  currentId: number
  credentials: CredentialStatusItem[]
}

export interface SystemVersionResponse {
  currentVersion: string
  latestVersion: string
  updateAvailable: boolean
  latestPublishedAt: string | null
  releaseNotesUrl: string | null
  buildType: string
  deploymentMode: string
  canUpdate: boolean
  canRollback: boolean
  canRestart: boolean
  updateHint: string
  checkedAt: string
  currentCommit?: string | null
  channel?: string | null
  latestJob?: SystemOperationJob | null
}

export type SchedulerPolicy = 'stable' | 'canary'
export type AccountStatus = 'normal' | 'banned' | 'rate_limited' | 'disabled'

export interface SystemOperationJob {
  jobId: string
  operation: string
  status: 'idle' | 'running' | 'succeeded' | 'failed' | 'rolled_back'
  targetVersion?: string | null
  currentVersion?: string | null
  startedAt?: string | null
  finishedAt?: string | null
  message: string
  canRetry: boolean
}

export interface PromptCacheConfigResponse {
  configured: boolean
  connected: boolean
  redisUrl?: string | null
  lastError?: string | null
}

export interface PromptCacheConfigRequest {
  redisUrl?: string | null
}

export type AdminTheme = 'light' | 'dark' | 'system'

export interface AdminSettingsResponse {
  theme: AdminTheme
  promptCache: PromptCacheConfigResponse
  accountsPageSize: number
  recordsPageSize: number
}

export interface AdminSettingsRequest {
  theme?: AdminTheme
  redisUrl?: string | null
  accountsPageSize?: number
  recordsPageSize?: number
}

export interface SchedulerModelOverrideConfig {
  maxModelConcurrency?: number | null
  minModelConcurrency?: number | null
  normal429BackoffInitialMs?: number | null
  normal429BackoffMaxMs?: number | null
  modelDecreaseRatio?: number | null
  modelIncreaseStep?: number | null
}

export interface SchedulerConfig {
  enabled: boolean
  requestBudgetMs: number
  queueTimeoutMs: number
  maxAttemptsPerRequest: number
  aggressiveRetry: boolean
  normal429BackoffInitialMs: number
  normal429BackoffMaxMs: number
  normal429BackoffMultiplier: number
  normal429JitterRatio: number
  modelDecreaseRatio: number
  modelIncreaseStep: number
  minModelConcurrency: number
  normal429AccountCooldownMs: number
  hedgeEnabled: boolean
  hedgeDelayMs: number
  hedgeMaxExtraPerRequest: number
  softFallbackEnabled: boolean
  suspiciousIsolationEnabled: boolean
  healthWeightedSchedulingEnabled: boolean
  suspiciousIsolationSeconds: number
  suspiciousStopRetry: boolean
  modelOverrides: Record<string, SchedulerModelOverrideConfig>
}

export interface SchedulerModelState {
  model: string
  upstreamModel: string
  schedulerPolicy: SchedulerPolicy
  window: number
  inflight: number
  successStreak: number
  backoffRemainingMs: number
  nextBackoffMs: number
}

export interface SchedulerConfigResponse {
  config: SchedulerConfig
  models: SchedulerModelState[]
}

// 单个凭据状态
export interface CredentialStatusItem {
  id: number
  priority: number
  disabled: boolean
  failureCount: number
  isCurrent: boolean
  expiresAt: string | null
  authMethod: string | null
  hasProfileArn: boolean
  email?: string
  refreshTokenHash?: string
  apiKeyHash?: string
  maskedApiKey?: string
  subscriptionTitle?: string | null
  availableModels?: string[] | null
  cachedBalance?: CachedBalanceStatus | null
  successCount: number
  lastUsedAt: string | null
  hasProxy: boolean
  proxyUrl?: string
  proxyMode?: 'pool' | string
  proxyId?: number
  proxyName?: string
  proxyStatus?: string
  schedulerPolicy?: SchedulerPolicy
  refreshFailureCount: number
  disabledReason?: string
  accountStatus: AccountStatus
  endpoint: string
  dispatchState: 'ready' | 'saturated' | 'cooldown' | 'blocked' | 'disabled'
  currentConcurrent: number
  maxConcurrent: number
  cooldownRemainingMs?: number
  lastRateLimitKind?: 'normal_429' | 'suspicious_activity' | 'refresh_429'
  recent429Count: number
  recentSuspiciousCount: number
  stickySessionCount: number
  stickyDetached: boolean
  dispatchPath?: 'preferred' | 'sticky' | 'balanced' | 'soft_fallback'
  softFallbackEligible: boolean
  lastSoftFallbackAt?: string | null
  suspiciousIsolated: boolean
  isolationRemainingMs?: number
  healthScore: number
  dispatchWeight: number
  weightReason: string
}

// 余额响应
export interface BalanceResponse {
  id: number
  subscriptionTitle: string | null
  currentUsage: number
  usageLimit: number
  remaining: number
  usagePercentage: number
  nextResetAt: number | null
}

export interface AvailableModelsResponse {
  id: number
  availableModels: string[]
}

export interface CredentialEmailResponse {
  id: number
  email: string
}

export interface CachedBalanceStatus {
  cachedAt: string
  fresh: boolean
  balance: BalanceResponse
}

export interface DiagnosticsBucket {
  key: string
  count: number
}

export interface DiagnosticsSummaryResponse {
  totalRequests: number
  successRequests: number
  failedRequests: number
  rateLimitedRequests: number
  suspiciousRequests: number
  averageDurationMs: number
  p50DurationMs: number
  p90DurationMs: number
  p99DurationMs: number
  inputTokens: number
  outputTokens: number
  cacheCreationInputTokens: number
  cacheReadInputTokens: number
  uncachedInputTokens: number
  modelRank: DiagnosticsBucket[]
  credentialRank: DiagnosticsBucket[]
  errorRank: DiagnosticsBucket[]
  timeBuckets: DiagnosticsTimeBucket[]
  credentialTimeBuckets: DiagnosticsCredentialTimeBucket[]
  latencyBuckets: DiagnosticsBucket[]
  credentialPerformance: DiagnosticsPerformanceItem[]
  modelPerformance: DiagnosticsPerformanceItem[]
}

export interface DiagnosticsTimeBucket {
  key: string
  totalRequests: number
  successRequests: number
  failedRequests: number
  rateLimitedRequests: number
  averageDurationMs: number
  inputTokens: number
  outputTokens: number
  cacheReadInputTokens: number
}

export interface DiagnosticsCredentialTimeBucket {
  key: string
  credentialId: number
  totalRequests: number
}

export interface DiagnosticsPerformanceItem {
  key: string
  totalRequests: number
  successRequests: number
  failedRequests: number
  rateLimitedRequests: number
  averageDurationMs: number
  inputTokens: number
  outputTokens: number
}

export interface RequestDiagnosticEntry {
  requestId: string
  startedAt: string
  finishedAt: string
  durationMs: number
  originalModel?: string | null
  mappedModel?: string | null
  credentialId?: number | null
  proxyName?: string | null
  dispatchPath?: 'preferred' | 'sticky' | 'balanced' | 'soft_fallback' | string | null
  stickyHit: boolean
  stickyDetached: boolean
  sessionHash?: string | null
  success: boolean
  upstreamStatus?: number | null
  upstreamErrorCode?: string | null
  upstreamMessageShort?: string | null
  rateLimitKind?: 'normal_429' | 'suspicious_activity' | 'refresh_429' | string | null
  cooldownMs?: number | null
  cooldownUntil?: string | null
  inputTokens?: number | null
  outputTokens?: number | null
  cacheCreationInputTokens?: number | null
  cacheReadInputTokens?: number | null
  uncachedInputTokens?: number | null
}

export interface DiagnosticsRequestsResponse {
  items: RequestDiagnosticEntry[]
  nextCursor?: number | null
  total: number
}

export interface DiagnosticsFilters {
  since?: string
  until?: string
  credentialId?: number
  model?: string
  success?: boolean
  keyword?: string
  rateLimitOnly?: boolean
  rateLimitKind?: string
  dispatchPath?: string
  limit?: number
  cursor?: number
}

export interface DiagnosticsCliResponse {
  command: string
}

export interface ProxyListItem {
  id: number
  name: string
  protocol: 'http' | 'https' | 'socks5' | string
  host: string
  port: number
  username?: string | null
  hasPassword: boolean
  disabled: boolean
  lastTestedAt?: string | null
  lastTestStatus?: 'ok' | 'failed' | 'unknown' | string | null
  lastLatencyMs?: number | null
  lastError?: string | null
  qualityCheckedAt?: string | null
  qualityScore?: number | null
  qualityGrade?: string | null
  exitIp?: string | null
  country?: string | null
  city?: string | null
  qualityError?: string | null
  accountCount: number
  isDefault: boolean
  createdAt: string
  updatedAt: string
}

export interface ProxyListResponse {
  total: number
  enabledCount: number
  proxies: ProxyListItem[]
}

export interface ProxyUpsertRequest {
  name: string
  protocol: string
  host: string
  port: number
  username?: string | null
  password?: string | null
  disabled: boolean
}

export interface BatchIdsRequest {
  ids: number[]
}

export interface BatchDisabledRequest extends BatchIdsRequest {
  disabled: boolean
}

export interface BatchCredentialUpdateRequest extends BatchIdsRequest {
  priority?: number
  maxConcurrent?: number
  disabled?: boolean
  schedulerPolicy?: SchedulerPolicy
}

export interface BatchOperationResponse {
  successCount: number
  failCount: number
  messages: string[]
}

export interface BatchBalanceResponse {
  successCount: number
  failCount: number
  balances: BalanceResponse[]
  messages: string[]
}

// 成功响应
export interface SuccessResponse {
  success: boolean
  message: string
}

// 错误响应
export interface AdminErrorResponse {
  error: {
    type: string
    message: string
  }
}

// 请求类型
export interface SetDisabledRequest {
  disabled: boolean
}

export interface SetPriorityRequest {
  priority: number
}

export interface SetMaxConcurrentRequest {
  maxConcurrent: number
}

export interface CredentialTestRequest {
  modelId: string
  prompt?: string
}

export interface CredentialTestEvent {
  type: 'test_start' | 'content' | 'tool_use' | 'context_usage' | 'upstream_error' | 'upstream_exception' | 'test_complete'
  accountId?: number
  model?: string
  dispatchPath?: 'preferred' | 'sticky' | 'balanced' | 'soft_fallback'
  usedSoftFallback?: boolean
  accountStateAtStart?: string
  text?: string
  name?: string
  input?: string
  stop?: boolean
  percentage?: number
  code?: string
  message?: string
  exceptionType?: string
  success?: boolean
  summary?: string
}

// 添加凭据请求
export interface AddCredentialRequest {
  refreshToken?: string
  authMethod?: 'social' | 'idc' | 'api_key'
  clientId?: string
  clientSecret?: string
  priority?: number
  maxConcurrent?: number
  authRegion?: string
  apiRegion?: string
  machineId?: string
  email?: string
  kiroApiKey?: string
  endpoint?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}

export interface SystemUpdateRequest {
  version?: string
}

export interface SystemRollbackRequest {
  backupName?: string
}
