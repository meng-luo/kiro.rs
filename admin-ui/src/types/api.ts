// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number
  available: number
  currentId: number
  credentials: CredentialStatusItem[]
}

export interface SystemVersionResponse {
  currentVersion: string
  latestVersion: string
  updateAvailable: boolean
  latestPublishedAt: string | null
  releaseNotesUrl: string | null
  deploymentMode: string
  canSelfUpdate: boolean
  updateHint: string
  checkedAt: string
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
  successCount: number
  lastUsedAt: string | null
  hasProxy: boolean
  proxyUrl?: string
  refreshFailureCount: number
  disabledReason?: string
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
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
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
