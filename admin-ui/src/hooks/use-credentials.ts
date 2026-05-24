import { useEffect } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  getCredentials,
  streamCredentials,
  setCredentialDisabled,
  setCredentialMaxConcurrent,
  setCredentialPriority,
  recoverCredential,
  resetCredentialFailure,
  forceRefreshToken,
  refreshCredentialModels,
  refreshCredentialEmail,
  getCredentialBalance,
  addCredential,
  deleteCredential,
  getDiagnosticsCli,
  getDiagnosticsRequest,
  getDiagnosticsRequests,
  getDiagnosticsSummary,
  getLoadBalancingMode,
  setLoadBalancingMode,
  getPromptCacheConfig,
  setPromptCacheConfig,
  getAdminSettings,
  setAdminSettings,
  getSchedulerConfig,
  setSchedulerConfig,
  getProxies,
  createProxy,
  updateProxy,
  deleteProxy,
  testProxy,
  batchTestProxies,
  batchDeleteProxies,
  batchQualityCheckProxies,
  getProxyAccounts,
  batchSetCredentialDisabled,
  batchResetCredentials,
  batchRefreshCredentials,
  batchRefreshBalances,
  batchDeleteCredentials,
  batchUpdateCredentials,
  getSystemVersion,
  checkSystemVersion,
  updateSystemVersion,
  rollbackSystemVersion,
  restartSystem,
  getSystemJob,
} from '@/api/credentials'
import type {
  AddCredentialRequest,
  AdminSettingsResponse,
  AdminSettingsRequest,
  SchedulerConfig,
  SchedulerConfigResponse,
  BatchCredentialUpdateRequest,
  BatchDisabledRequest,
  BatchIdsRequest,
  DiagnosticsFilters,
  PromptCacheConfigRequest,
  ProxyUpsertRequest,
  SystemRollbackRequest,
  SystemUpdateRequest,
} from '@/types/api'

// 查询凭据列表
export function useCredentials() {
  return useQuery({
    queryKey: ['credentials'],
    queryFn: getCredentials,
    refetchInterval: 60000, // SSE 断线时兜底刷新
  })
}

export function useCredentialsStream(enabled = true) {
  const queryClient = useQueryClient()

  useEffect(() => {
    if (!enabled) return

    let stopped = false
    let retryTimer: number | undefined
    let retryCount = 0
    let controller: AbortController | null = null

    const connect = () => {
      if (stopped) return
      controller = new AbortController()
      streamCredentials((data) => {
        retryCount = 0
        queryClient.setQueryData(['credentials'], data)
      }, controller.signal).catch((error) => {
        if (stopped || controller?.signal.aborted) return
        console.warn('账号列表推送已断开，准备重连:', error)
        const delay = Math.min(30000, 1000 * 2 ** retryCount)
        retryCount += 1
        retryTimer = window.setTimeout(connect, delay)
      })
    }

    connect()

    return () => {
      stopped = true
      controller?.abort()
      if (retryTimer) window.clearTimeout(retryTimer)
    }
  }, [enabled, queryClient])
}

export function useDiagnosticsSummary(filters: DiagnosticsFilters) {
  return useQuery({
    queryKey: ['diagnostics-summary', filters],
    queryFn: () => getDiagnosticsSummary(filters),
    refetchInterval: 30000,
  })
}

export function useDiagnosticsRequests(filters: DiagnosticsFilters) {
  return useQuery({
    queryKey: ['diagnostics-requests', filters],
    queryFn: () => getDiagnosticsRequests(filters),
    refetchInterval: 30000,
  })
}

export function useDiagnosticsCli(filters: DiagnosticsFilters) {
  return useQuery({
    queryKey: ['diagnostics-cli', filters],
    queryFn: () => getDiagnosticsCli(filters),
  })
}

export function useDiagnosticsRequest(requestId: string | null) {
  return useQuery({
    queryKey: ['diagnostics-request', requestId],
    queryFn: () => getDiagnosticsRequest(requestId!),
    enabled: !!requestId,
  })
}

// 查询凭据余额
export function useCredentialBalance(id: number | null) {
  return useQuery({
    queryKey: ['credential-balance', id],
    queryFn: () => getCredentialBalance(id!),
    enabled: id !== null,
    retry: false, // 余额查询失败时不重试（避免重复请求被封禁的账号）
  })
}

// 设置禁用状态
export function useSetDisabled() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, disabled }: { id: number; disabled: boolean }) =>
      setCredentialDisabled(id, disabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 设置优先级
export function useSetPriority() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, priority }: { id: number; priority: number }) =>
      setCredentialPriority(id, priority),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useSetMaxConcurrent() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, maxConcurrent }: { id: number; maxConcurrent: number }) =>
      setCredentialMaxConcurrent(id, maxConcurrent),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useRecoverCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => recoverCredential(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 重置失败计数
export function useResetFailure() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => resetCredentialFailure(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 强制刷新 Token
export function useForceRefreshToken() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => forceRefreshToken(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useRefreshCredentialModels() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => refreshCredentialModels(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useRefreshCredentialEmail() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => refreshCredentialEmail(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 添加新凭据
export function useAddCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: AddCredentialRequest) => addCredential(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 删除凭据
export function useDeleteCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteCredential(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 获取负载均衡模式
export function useLoadBalancingMode() {
  return useQuery({
    queryKey: ['loadBalancingMode'],
    queryFn: getLoadBalancingMode,
  })
}

// 设置负载均衡模式
export function useSetLoadBalancingMode() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setLoadBalancingMode,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['loadBalancingMode'] })
    },
  })
}

export function usePromptCacheConfig() {
  return useQuery({
    queryKey: ['promptCacheConfig'],
    queryFn: getPromptCacheConfig,
  })
}

export function useSetPromptCacheConfig() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: PromptCacheConfigRequest) => setPromptCacheConfig(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['promptCacheConfig'] })
    },
  })
}

export function useAdminSettings() {
  return useQuery({
    queryKey: ['adminSettings'],
    queryFn: getAdminSettings,
  })
}

export function useSetAdminSettings() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: AdminSettingsRequest) => setAdminSettings(payload),
    onSuccess: (response: AdminSettingsResponse) => {
      queryClient.setQueryData(['adminSettings'], response)
      queryClient.invalidateQueries({ queryKey: ['adminSettings'] })
      queryClient.invalidateQueries({ queryKey: ['promptCacheConfig'] })
    },
  })
}

export function useSchedulerConfig() {
  return useQuery({
    queryKey: ['schedulerConfig'],
    queryFn: getSchedulerConfig,
    refetchInterval: 5000,
  })
}

export function useSetSchedulerConfig() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: SchedulerConfig) => setSchedulerConfig(payload),
    onSuccess: (response: SchedulerConfigResponse) => {
      queryClient.setQueryData(['schedulerConfig'], response)
      queryClient.invalidateQueries({ queryKey: ['schedulerConfig'] })
    },
  })
}

export function useProxies() {
  return useQuery({
    queryKey: ['proxies'],
    queryFn: getProxies,
    refetchInterval: 60000,
  })
}

export function useCreateProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: ProxyUpsertRequest) => createProxy(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
    },
  })
}

export function useUpdateProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, payload }: { id: number; payload: ProxyUpsertRequest }) => updateProxy(id, payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useDeleteProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
    },
  })
}

export function useTestProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => testProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBatchTestProxies() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchIdsRequest) => batchTestProxies(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
    },
  })
}

export function useBatchDeleteProxies() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchIdsRequest) => batchDeleteProxies(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBatchQualityCheckProxies() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchIdsRequest) => batchQualityCheckProxies(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
    },
  })
}

export function useProxyAccounts(id: number | null) {
  return useQuery({
    queryKey: ['proxyAccounts', id],
    queryFn: () => getProxyAccounts(id!),
    enabled: id !== null,
  })
}

export function useBatchSetDisabled() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchDisabledRequest) => batchSetCredentialDisabled(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBatchResetCredentials() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchIdsRequest) => batchResetCredentials(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBatchRefreshCredentials() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchIdsRequest) => batchRefreshCredentials(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBatchRefreshBalances() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchIdsRequest) => batchRefreshBalances(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBatchDeleteCredentials() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchIdsRequest) => batchDeleteCredentials(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBatchUpdateCredentials() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload: BatchCredentialUpdateRequest) => batchUpdateCredentials(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      queryClient.invalidateQueries({ queryKey: ['proxies'] })
    },
  })
}

export function useSystemVersion() {
  return useQuery({
    queryKey: ['systemVersion'],
    queryFn: getSystemVersion,
    refetchInterval: 60000,
  })
}

export function useCheckSystemVersion() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: checkSystemVersion,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['systemVersion'] })
    },
  })
}

export function useUpdateSystemVersion() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload?: SystemUpdateRequest) => updateSystemVersion(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['systemVersion'] })
    },
  })
}

export function useRollbackSystemVersion() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (payload?: SystemRollbackRequest) => rollbackSystemVersion(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['systemVersion'] })
    },
  })
}

export function useRestartSystem() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: restartSystem,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['systemVersion'] })
    },
  })
}

export function useSystemJob(jobId: string | null, enabled = true) {
  return useQuery({
    queryKey: ['systemJob', jobId],
    queryFn: () => getSystemJob(jobId!),
    enabled: enabled && Boolean(jobId),
    refetchInterval: (query) => {
      const status = query.state.data?.status
      if (!status) return 2000
      return status === 'running' ? 2000 : false
    },
  })
}
