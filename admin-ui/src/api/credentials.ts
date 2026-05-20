import axios from 'axios'
import { storage } from '@/lib/storage'
import type {
  CredentialsStatusResponse,
  BalanceResponse,
  SuccessResponse,
  SetMaxConcurrentRequest,
  SetDisabledRequest,
  SetPriorityRequest,
  AddCredentialRequest,
  AddCredentialResponse,
  CredentialTestRequest,
  DiagnosticsCliResponse,
  DiagnosticsFilters,
  DiagnosticsRequestsResponse,
  DiagnosticsSummaryResponse,
  SystemVersionResponse,
  SystemOperationJob,
  SystemUpdateRequest,
  SystemRollbackRequest,
  PromptCacheConfigResponse,
  PromptCacheConfigRequest,
} from '@/types/api'

// 创建 axios 实例
const api = axios.create({
  baseURL: '/api/admin',
  headers: {
    'Content-Type': 'application/json',
  },
})

// 请求拦截器添加 API Key
api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey()
  if (apiKey) {
    config.headers['x-api-key'] = apiKey
  }
  return config
})

// 获取所有凭据状态
export async function getCredentials(): Promise<CredentialsStatusResponse> {
  const { data } = await api.get<CredentialsStatusResponse>('/credentials')
  return data
}

export async function streamCredentials(
  onMessage: (data: CredentialsStatusResponse) => void,
  signal: AbortSignal
): Promise<void> {
  const apiKey = storage.getApiKey()
  const response = await fetch('/api/admin/credentials/stream', {
    headers: {
      ...(apiKey ? { 'x-api-key': apiKey } : {}),
    },
    signal,
  })

  if (!response.ok || !response.body) {
    throw new Error(`账号列表推送连接失败: HTTP ${response.status}`)
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''

  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    const chunks = buffer.split('\n\n')
    buffer = chunks.pop() ?? ''

    for (const chunk of chunks) {
      const event = chunk.split('\n').find((line) => line.startsWith('event:'))?.slice(6).trim()
      const data = chunk.split('\n').find((line) => line.startsWith('data:'))?.slice(5).trim()
      if (event !== 'credentials' || !data) continue
      onMessage(JSON.parse(data) as CredentialsStatusResponse)
    }
  }
}

function diagnosticsParams(filters: DiagnosticsFilters) {
  const params = new URLSearchParams()
  Object.entries(filters).forEach(([key, value]) => {
    if (value === undefined || value === null || value === '') return
    params.set(key, String(value))
  })
  return params
}

export async function getDiagnosticsSummary(filters: DiagnosticsFilters): Promise<DiagnosticsSummaryResponse> {
  const params = diagnosticsParams(filters)
  const { data } = await api.get<DiagnosticsSummaryResponse>(`/diagnostics/summary?${params.toString()}`)
  return data
}

export async function getDiagnosticsRequests(filters: DiagnosticsFilters): Promise<DiagnosticsRequestsResponse> {
  const params = diagnosticsParams(filters)
  const { data } = await api.get<DiagnosticsRequestsResponse>(`/diagnostics/requests?${params.toString()}`)
  return data
}

export async function getDiagnosticsCli(filters: DiagnosticsFilters): Promise<DiagnosticsCliResponse> {
  const params = diagnosticsParams(filters)
  const { data } = await api.get<DiagnosticsCliResponse>(`/diagnostics/cli?${params.toString()}`)
  return data
}

// 设置凭据禁用状态
export async function setCredentialDisabled(
  id: number,
  disabled: boolean
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/disabled`,
    { disabled } as SetDisabledRequest
  )
  return data
}

// 设置凭据优先级
export async function setCredentialPriority(
  id: number,
  priority: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/priority`,
    { priority } as SetPriorityRequest
  )
  return data
}

export async function setCredentialMaxConcurrent(
  id: number,
  maxConcurrent: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/max-concurrent`,
    { maxConcurrent } as SetMaxConcurrentRequest
  )
  return data
}

export async function recoverCredential(id: number): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/recover`)
  return data
}

// 重置失败计数
export async function resetCredentialFailure(
  id: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/reset`)
  return data
}

// 强制刷新 Token
export async function forceRefreshToken(
  id: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/refresh`)
  return data
}

// 获取凭据余额
export async function getCredentialBalance(id: number): Promise<BalanceResponse> {
  const { data } = await api.get<BalanceResponse>(`/credentials/${id}/balance`)
  return data
}

// 添加新凭据
export async function addCredential(
  req: AddCredentialRequest
): Promise<AddCredentialResponse> {
  const { data } = await api.post<AddCredentialResponse>('/credentials', req)
  return data
}

// 删除凭据
export async function deleteCredential(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/credentials/${id}`)
  return data
}

// 获取负载均衡模式
export async function getLoadBalancingMode(): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.get<{ mode: 'priority' | 'balanced' }>('/config/load-balancing')
  return data
}

// 设置负载均衡模式
export async function setLoadBalancingMode(mode: 'priority' | 'balanced'): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.put<{ mode: 'priority' | 'balanced' }>('/config/load-balancing', { mode })
  return data
}

export async function getPromptCacheConfig(): Promise<PromptCacheConfigResponse> {
  const { data } = await api.get<PromptCacheConfigResponse>('/config/prompt-cache')
  return data
}

export async function setPromptCacheConfig(
  payload: PromptCacheConfigRequest
): Promise<PromptCacheConfigResponse> {
  const { data } = await api.put<PromptCacheConfigResponse>('/config/prompt-cache', payload)
  return data
}

export async function getSystemVersion(): Promise<SystemVersionResponse> {
  const { data } = await api.get<SystemVersionResponse>('/system/version')
  return data
}

export async function checkSystemVersion(): Promise<SystemVersionResponse> {
  const { data } = await api.post<SystemVersionResponse>('/system/version/check')
  return data
}

export async function updateSystemVersion(
  payload: SystemUpdateRequest = {}
): Promise<SystemOperationJob> {
  const { data } = await api.post<SystemOperationJob>('/system/update', payload)
  return data
}

export async function rollbackSystemVersion(
  payload: SystemRollbackRequest = {}
): Promise<SystemOperationJob> {
  const { data } = await api.post<SystemOperationJob>('/system/rollback', payload)
  return data
}

export async function restartSystem(): Promise<SystemOperationJob> {
  const { data } = await api.post<SystemOperationJob>('/system/restart')
  return data
}

export async function getSystemJob(jobId: string): Promise<SystemOperationJob> {
  const { data } = await api.get<SystemOperationJob>(`/system/update/jobs/${jobId}`)
  return data
}

export async function testCredential(
  id: number,
  payload: CredentialTestRequest
): Promise<Response> {
  const apiKey = storage.getApiKey()
  return fetch(`/api/admin/credentials/${id}/test`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(apiKey ? { 'x-api-key': apiKey } : {}),
    },
    body: JSON.stringify(payload),
  })
}
