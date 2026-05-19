import { useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowUpRight,
  BadgeInfo,
  CheckCircle2,
  Download,
  FileUp,
  History,
  LogOut,
  Moon,
  Plus,
  Power,
  RefreshCw,
  RotateCcw,
  Server,
  Sun,
  Trash2,
  Upload,
} from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { storage } from '@/lib/storage'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Checkbox } from '@/components/ui/checkbox'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { BatchImportDialog } from '@/components/batch-import-dialog'
import { KamImportDialog } from '@/components/kam-import-dialog'
import { BatchVerifyDialog, type VerifyResult } from '@/components/batch-verify-dialog'
import { CredentialRow } from '@/components/credential-row'
import {
  useCheckSystemVersion,
  useCredentials,
  useDeleteCredential,
  useLoadBalancingMode,
  useResetFailure,
  useRestartSystem,
  useRollbackSystemVersion,
  useSetLoadBalancingMode,
  useSystemJob,
  useSystemVersion,
  useUpdateSystemVersion,
} from '@/hooks/use-credentials'
import { forceRefreshToken, getCredentialBalance } from '@/api/credentials'
import { cn, extractErrorMessage } from '@/lib/utils'
import type { BalanceResponse, CredentialStatusItem, SystemOperationJob } from '@/types/api'

function deploymentModeLabel(mode?: string) {
  switch (mode) {
    case 'binary':
      return '二进制部署'
    case 'docker':
      return '容器部署'
    case 'file':
      return '文件部署'
    default:
      return mode || '未知'
  }
}

function buildTypeLabel(mode?: string) {
  switch (mode) {
    case 'release':
      return '发布构建'
    case 'source':
      return '源码构建'
    default:
      return mode || '未知'
  }
}

function channelLabel(channel?: string | null) {
  switch (channel) {
    case 'stable':
      return '稳定版'
    case 'beta':
      return '预览版'
    default:
      return channel || '未设置'
  }
}

function operationLabel(job?: SystemOperationJob | null) {
  switch (job?.operation) {
    case 'update':
      return '更新'
    case 'rollback':
      return '回滚'
    case 'restart':
      return '重启'
    default:
      return '系统任务'
  }
}

function updateActionLabel(mode?: string) {
  return mode === 'docker' ? '更新测试实例' : '更新到最新'
}

function updateActionTitle(mode?: string, canUpdate?: boolean) {
  if (!canUpdate) {
    return mode === 'docker' ? '当前未配置测试实例一键更新' : '当前构建不支持在线更新'
  }
  return mode === 'docker' ? '拉取最新测试镜像并重建测试实例' : '下载并准备更新'
}

function TableHead({
  children,
  className,
}: {
  children: React.ReactNode
  className?: string
}) {
  return (
    <th className={cn('bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground whitespace-nowrap', className)}>
      {children}
    </th>
  )
}

function StatCard({
  label,
  value,
  valueClassName,
}: {
  label: string
  value: string | number
  valueClassName?: string
}) {
  return (
    <Card className="rounded-md">
      <CardHeader className="pb-2">
        <CardTitle className="truncate text-xs font-medium text-muted-foreground">{label}</CardTitle>
      </CardHeader>
      <CardContent>
        <div className={cn('truncate whitespace-nowrap text-xl font-semibold', valueClassName)}>{value}</div>
      </CardContent>
    </Card>
  )
}

interface DashboardProps {
  onLogout: () => void
}

export function Dashboard({ onLogout }: DashboardProps) {
  const [selectedCredentialId, setSelectedCredentialId] = useState<number | null>(null)
  const [balanceDialogOpen, setBalanceDialogOpen] = useState(false)
  const [versionDialogOpen, setVersionDialogOpen] = useState(false)
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [batchImportDialogOpen, setBatchImportDialogOpen] = useState(false)
  const [kamImportDialogOpen, setKamImportDialogOpen] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set())
  const [verifyDialogOpen, setVerifyDialogOpen] = useState(false)
  const [verifying, setVerifying] = useState(false)
  const [verifyProgress, setVerifyProgress] = useState({ current: 0, total: 0 })
  const [verifyResults, setVerifyResults] = useState<Map<number, VerifyResult>>(new Map())
  const [balanceMap, setBalanceMap] = useState<Map<number, BalanceResponse>>(new Map())
  const [loadingBalanceIds, setLoadingBalanceIds] = useState<Set<number>>(new Set())
  const [queryingInfo, setQueryingInfo] = useState(false)
  const [queryInfoProgress, setQueryInfoProgress] = useState({ current: 0, total: 0 })
  const [batchRefreshing, setBatchRefreshing] = useState(false)
  const [batchRefreshProgress, setBatchRefreshProgress] = useState({ current: 0, total: 0 })
  const [activeJobId, setActiveJobId] = useState<string | null>(null)
  const cancelVerifyRef = useRef(false)
  const [currentPage, setCurrentPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [darkMode, setDarkMode] = useState(() => {
    if (typeof window !== 'undefined') {
      const savedTheme = storage.getTheme()
      if (savedTheme) return savedTheme === 'dark'
      return document.documentElement.classList.contains('dark')
    }
    return false
  })

  const queryClient = useQueryClient()
  const { data, isLoading, error, refetch } = useCredentials()
  const { mutate: deleteCredential } = useDeleteCredential()
  const { mutate: resetFailure } = useResetFailure()
  const { data: loadBalancingData, isLoading: isLoadingMode } = useLoadBalancingMode()
  const { mutate: setLoadBalancingMode, isPending: isSettingMode } = useSetLoadBalancingMode()
  const { data: systemVersion, isLoading: isLoadingVersion } = useSystemVersion()
  const { mutate: checkSystemVersion, isPending: isCheckingVersion } = useCheckSystemVersion()
  const { mutate: updateSystemVersion, isPending: isUpdatingSystem } = useUpdateSystemVersion()
  const { mutate: rollbackSystemVersion, isPending: isRollingBackSystem } = useRollbackSystemVersion()
  const { mutate: restartSystem, isPending: isRestartingSystem } = useRestartSystem()
  const { data: activeSystemJob } = useSystemJob(activeJobId, versionDialogOpen)

  const latestSystemJob = activeSystemJob || systemVersion?.latestJob || null
  const versionActionsBusy = isUpdatingSystem || isRollingBackSystem || isRestartingSystem

  const credentials = data?.credentials || []
  const totalPages = Math.max(1, Math.ceil(credentials.length / pageSize))
  const startIndex = (currentPage - 1) * pageSize
  const endIndex = Math.min(startIndex + pageSize, credentials.length)
  const currentCredentials = credentials.slice(startIndex, endIndex)
  const disabledCredentialCount = credentials.filter((credential) => credential.disabled).length
  const enabledCredentialCount = data?.enabledCount ?? credentials.filter((credential) => !credential.disabled).length
  const schedulableCredentialCount = data?.schedulableCount ?? data?.available ?? credentials.filter((credential) => credential.dispatchState === 'ready').length
  const cooldownCredentialCount = credentials.filter((credential) => credential.dispatchState === 'cooldown').length
  const saturatedCredentialCount = credentials.filter((credential) => credential.dispatchState === 'saturated').length
  const blockedCredentialCount = credentials.filter((credential) => credential.dispatchState === 'blocked' && !credential.disabled).length
  const currentPageIds = currentCredentials.map((credential) => credential.id)
  const isCurrentPageAllSelected = currentPageIds.length > 0 && currentPageIds.every((id) => selectedIds.has(id))

  const jobStatusMeta = useMemo(() => {
    const status = latestSystemJob?.status
    switch (status) {
      case 'running':
        return { text: '执行中', variant: 'warning' as const }
      case 'succeeded':
        return { text: '已完成', variant: 'success' as const }
      case 'rolled_back':
        return { text: '已回滚', variant: 'outline' as const }
      case 'failed':
        return { text: '失败', variant: 'destructive' as const }
      default:
        return { text: '暂无任务', variant: 'secondary' as const }
    }
  }, [latestSystemJob?.status])

  useEffect(() => {
    if (currentPage > totalPages) {
      setCurrentPage(totalPages)
    }
  }, [currentPage, totalPages])

  useEffect(() => {
    if (systemVersion?.latestJob?.jobId) {
      setActiveJobId((prev) => prev ?? systemVersion.latestJob?.jobId ?? null)
    }
  }, [systemVersion?.latestJob?.jobId])

  useEffect(() => {
    const validIds = new Set(credentials.map((credential) => credential.id))

    setSelectedIds((prev) => {
      if (prev.size === 0) return prev
      const next = new Set<number>()
      prev.forEach((id) => {
        if (validIds.has(id)) next.add(id)
      })
      return next.size === prev.size ? prev : next
    })

    if (credentials.length === 0) {
      setBalanceMap(new Map())
      setLoadingBalanceIds(new Set())
      return
    }

    setBalanceMap((prev) => {
      const next = new Map<number, BalanceResponse>()
      prev.forEach((value, id) => {
        if (validIds.has(id)) next.set(id, value)
      })
      return next.size === prev.size ? prev : next
    })

    setLoadingBalanceIds((prev) => {
      if (prev.size === 0) return prev
      const next = new Set<number>()
      prev.forEach((id) => {
        if (validIds.has(id)) next.add(id)
      })
      return next.size === prev.size ? prev : next
    })
  }, [credentials])

  const toggleDarkMode = () => {
    setDarkMode((current) => {
      const next = !current
      document.documentElement.classList.toggle('dark', next)
      storage.setTheme(next ? 'dark' : 'light')
      return next
    })
  }

  const handleViewBalance = (id: number) => {
    setSelectedCredentialId(id)
    setBalanceDialogOpen(true)
  }

  const handleRefresh = () => {
    refetch()
    toast.success('已刷新账号列表')
  }

  const handleLogout = () => {
    storage.removeApiKey()
    queryClient.clear()
    onLogout()
  }

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const handleToggleSelectCurrentPage = () => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (isCurrentPageAllSelected) {
        currentPageIds.forEach((id) => next.delete(id))
      } else {
        currentPageIds.forEach((id) => next.add(id))
      }
      return next
    })
  }

  const deselectAll = () => {
    setSelectedIds(new Set())
  }

  const runDeleteIds = async (ids: number[]) => {
    let successCount = 0
    let failCount = 0

    for (const id of ids) {
      try {
        await new Promise<void>((resolve, reject) => {
          deleteCredential(id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: (err) => {
              failCount++
              reject(err)
            }
          })
        })
      } catch {
        // 错误已在 onError 中处理
      }
    }

    return { successCount, failCount }
  }

  const handleBatchDelete = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要删除的账号')
      return
    }

    const ids = Array.from(selectedIds)
    const includesCurrent = credentials.some((credential) => credential.isCurrent && selectedIds.has(credential.id))
    const hint = includesCurrent ? '包含当前正在使用的账号，系统会自动切换到其他可用账号。' : ''

    if (!confirm(`确定要删除 ${ids.length} 个账号吗？删除后无法恢复。${hint}`)) {
      return
    }

    const { successCount, failCount } = await runDeleteIds(ids)

    if (failCount === 0) {
      toast.success(`成功删除 ${successCount} 个账号`)
    } else {
      toast.warning(`删除完成：成功 ${successCount} 个，失败 ${failCount} 个`)
    }
    deselectAll()
  }

  const handleBatchResetFailure = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要恢复的账号')
      return
    }

    const failedIds = Array.from(selectedIds).filter((id) => {
      const cred = credentials.find((item) => item.id === id)
      return cred && cred.failureCount > 0
    })

    if (failedIds.length === 0) {
      toast.error('选中的账号里没有异常项')
      return
    }

    let successCount = 0
    let failCount = 0

    for (const id of failedIds) {
      try {
        await new Promise<void>((resolve, reject) => {
          resetFailure(id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: () => {
              failCount++
              reject(new Error('reset failed'))
            }
          })
        })
      } catch {
        // 已计数
      }
    }

    if (failCount === 0) {
      toast.success(`成功恢复 ${successCount} 个账号`)
    } else {
      toast.warning(`恢复完成：成功 ${successCount} 个，失败 ${failCount} 个`)
    }
    deselectAll()
  }

  const handleBatchForceRefresh = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要刷新的账号')
      return
    }

    const enabledIds = Array.from(selectedIds).filter((id) => {
      const cred = credentials.find((item) => item.id === id)
      return cred && !cred.disabled
    })

    if (enabledIds.length === 0) {
      toast.error('选中的账号里没有可刷新的启用账号')
      return
    }

    setBatchRefreshing(true)
    setBatchRefreshProgress({ current: 0, total: enabledIds.length })

    let successCount = 0
    let failCount = 0

    for (let i = 0; i < enabledIds.length; i++) {
      try {
        await forceRefreshToken(enabledIds[i])
        successCount++
      } catch {
        failCount++
      }
      setBatchRefreshProgress({ current: i + 1, total: enabledIds.length })
    }

    setBatchRefreshing(false)
    queryClient.invalidateQueries({ queryKey: ['credentials'] })

    if (failCount === 0) {
      toast.success(`成功刷新 ${successCount} 个账号的 Token`)
    } else {
      toast.warning(`刷新完成：成功 ${successCount} 个，失败 ${failCount} 个`)
    }
    deselectAll()
  }

  const handleClearAll = async () => {
    const disabledCredentials = credentials.filter((credential) => credential.disabled)
    if (disabledCredentials.length === 0) {
      toast.error('没有可清除的已停用账号')
      return
    }

    if (!confirm(`确定要清除所有 ${disabledCredentials.length} 个已停用账号吗？删除后无法恢复。`)) {
      return
    }

    const { successCount, failCount } = await runDeleteIds(disabledCredentials.map((credential) => credential.id))
    if (failCount === 0) {
      toast.success(`成功清除 ${successCount} 个已停用账号`)
    } else {
      toast.warning(`清除完成：成功 ${successCount} 个，失败 ${failCount} 个`)
    }
    deselectAll()
  }

  const handleQueryCurrentPageInfo = async () => {
    const ids = currentCredentials.filter((credential) => !credential.disabled).map((credential) => credential.id)
    if (ids.length === 0) {
      toast.error('当前页没有可查询的启用账号')
      return
    }

    setQueryingInfo(true)
    setQueryInfoProgress({ current: 0, total: ids.length })

    let successCount = 0
    let failCount = 0

    for (let i = 0; i < ids.length; i++) {
      const id = ids[i]
      setLoadingBalanceIds((prev) => new Set(prev).add(id))

      try {
        const balance = await getCredentialBalance(id)
        successCount++
        setBalanceMap((prev) => {
          const next = new Map(prev)
          next.set(id, balance)
          return next
        })
      } catch {
        failCount++
      } finally {
        setLoadingBalanceIds((prev) => {
          const next = new Set(prev)
          next.delete(id)
          return next
        })
      }
      setQueryInfoProgress({ current: i + 1, total: ids.length })
    }

    setQueryingInfo(false)
    if (failCount === 0) {
      toast.success(`查询完成：成功 ${successCount}/${ids.length}`)
    } else {
      toast.warning(`查询完成：成功 ${successCount} 个，失败 ${failCount} 个`)
    }
  }

  const handleBatchVerify = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要验活的账号')
      return
    }

    setVerifying(true)
    cancelVerifyRef.current = false
    const ids = Array.from(selectedIds)
    setVerifyProgress({ current: 0, total: ids.length })

    let successCount = 0
    const initialResults = new Map<number, VerifyResult>()
    ids.forEach((id) => {
      initialResults.set(id, { id, status: 'pending' })
    })
    setVerifyResults(initialResults)
    setVerifyDialogOpen(true)

    for (let i = 0; i < ids.length; i++) {
      if (cancelVerifyRef.current) {
        toast.info('已取消验活')
        break
      }

      const id = ids[i]
      setVerifyResults((prev) => {
        const next = new Map(prev)
        next.set(id, { id, status: 'verifying' })
        return next
      })

      try {
        const balance = await getCredentialBalance(id)
        successCount++
        setVerifyResults((prev) => {
          const next = new Map(prev)
          next.set(id, { id, status: 'success', usage: `${balance.currentUsage}/${balance.usageLimit}` })
          return next
        })
      } catch (error) {
        setVerifyResults((prev) => {
          const next = new Map(prev)
          next.set(id, { id, status: 'failed', error: extractErrorMessage(error) })
          return next
        })
      }

      setVerifyProgress({ current: i + 1, total: ids.length })
      if (i < ids.length - 1 && !cancelVerifyRef.current) {
        await new Promise((resolve) => setTimeout(resolve, 2000))
      }
    }

    setVerifying(false)
    if (!cancelVerifyRef.current) {
      toast.success(`验活完成：成功 ${successCount}/${ids.length}`)
    }
  }

  const handleCancelVerify = () => {
    cancelVerifyRef.current = true
    setVerifying(false)
  }

  const handleToggleLoadBalancing = () => {
    const currentMode = loadBalancingData?.mode || 'priority'
    const newMode = currentMode === 'priority' ? 'balanced' : 'priority'
    setLoadBalancingMode(newMode, {
      onSuccess: () => {
        toast.success(newMode === 'priority' ? '已切回优先级模式' : '已切换到均衡负载模式')
      },
      onError: (error) => {
        toast.error(`切换失败: ${extractErrorMessage(error)}`)
      }
    })
  }

  const handleCheckVersion = () => {
    checkSystemVersion(undefined, {
      onSuccess: (response) => {
        setActiveJobId(response.latestJob?.jobId ?? null)
        toast.success('已刷新版本信息')
      },
      onError: (error) => {
        toast.error(`版本检查失败: ${extractErrorMessage(error)}`)
      }
    })
  }

  const handleStartUpdate = () => {
    updateSystemVersion(systemVersion?.updateAvailable ? { version: systemVersion.latestVersion } : {}, {
      onSuccess: (job) => {
        setActiveJobId(job.jobId)
        toast.success(systemVersion?.deploymentMode === 'docker' ? '已发起测试实例更新' : '已发起更新任务')
      },
      onError: (error) => {
        toast.error(`发起更新失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  const handleStartRollback = () => {
    rollbackSystemVersion({}, {
      onSuccess: (job) => {
        setActiveJobId(job.jobId)
        toast.success('已发起回滚任务')
      },
      onError: (error) => {
        toast.error(`发起回滚失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  const handleStartRestart = () => {
    restartSystem(undefined, {
      onSuccess: (job) => {
        setActiveJobId(job.jobId)
        toast.success('已发起重启任务')
      },
      onError: (error) => {
        toast.error(`发起重启失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  if (isLoading) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background">
        <div className="text-center">
          <div className="mx-auto mb-4 h-12 w-12 animate-spin rounded-full border-b-2 border-primary"></div>
          <p className="text-muted-foreground">加载中...</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background p-4">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6 text-center">
            <div className="mb-4 text-red-500">加载失败</div>
            <p className="mb-4 text-muted-foreground">{(error as Error).message}</p>
            <div className="space-x-2">
              <Button onClick={() => refetch()}>重试</Button>
              <Button variant="outline" onClick={handleLogout}>重新登录</Button>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className="min-h-screen bg-background">
      <header className="sticky top-0 z-50 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
        <div className="container flex h-14 items-center justify-between px-4 md:px-8">
          <div className="flex items-center gap-2">
            <Server className="h-5 w-5" />
            <span className="font-semibold">Kiro Admin</span>
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => setVersionDialogOpen(true)}
              disabled={isLoadingVersion}
              title="查看版本信息"
              className="max-w-[180px] truncate"
            >
              <BadgeInfo className="mr-2 h-4 w-4" />
              {isLoadingVersion ? '版本...' : (systemVersion ? systemVersion.currentVersion : '版本')}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={handleToggleLoadBalancing}
              disabled={isLoadingMode || isSettingMode}
              title="切换负载均衡模式"
              className="max-w-[180px] truncate"
            >
              {isLoadingMode ? '加载中...' : (loadBalancingData?.mode === 'priority' ? '优先级模式' : '均衡负载')}
            </Button>
            <Button variant="ghost" size="icon" onClick={toggleDarkMode}>
              {darkMode ? <Sun className="h-5 w-5" /> : <Moon className="h-5 w-5" />}
            </Button>
            <Button variant="ghost" size="icon" onClick={handleRefresh}>
              <RefreshCw className="h-5 w-5" />
            </Button>
            <Button variant="ghost" size="icon" onClick={handleLogout}>
              <LogOut className="h-5 w-5" />
            </Button>
          </div>
        </div>
      </header>

      <main className="container mx-auto space-y-6 px-4 py-6 md:px-8">
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4 xl:grid-cols-7">
          <StatCard label="账号总数" value={data?.total || 0} />
          <StatCard label="启用账号" value={enabledCredentialCount} valueClassName="text-green-600" />
          <StatCard label="可调度" value={schedulableCredentialCount} valueClassName="text-emerald-600" />
          <StatCard label="当前活跃" value={data?.currentId ? `#${data.currentId}` : '-'} />
          <StatCard label="冷却中" value={cooldownCredentialCount} valueClassName="text-blue-600" />
          <StatCard label="并发已满" value={saturatedCredentialCount} valueClassName="text-yellow-600" />
          <StatCard label="本地阻塞" value={blockedCredentialCount} valueClassName="text-orange-600" />
        </div>

        <Card className="rounded-md">
          <CardHeader className="gap-4">
            <div className="flex items-center justify-between gap-4">
              <div className="min-w-0">
                <CardTitle className="truncate text-lg">账号管理</CardTitle>
                <div className="mt-1 flex items-center gap-2 text-sm text-muted-foreground">
                  <span className="truncate">适合多账号运维的紧凑列表视图</span>
                  {selectedIds.size > 0 ? <Badge variant="secondary" className="whitespace-nowrap">已选 {selectedIds.size}</Badge> : null}
                </div>
              </div>
              <div className="flex items-center gap-2 whitespace-nowrap">
                <Button onClick={() => setKamImportDialogOpen(true)} size="sm" variant="outline">
                  <FileUp className="mr-2 h-4 w-4" />
                  KAM 导入
                </Button>
                <Button onClick={() => setBatchImportDialogOpen(true)} size="sm" variant="outline">
                  <Upload className="mr-2 h-4 w-4" />
                  批量导入
                </Button>
                <Button onClick={() => setAddDialogOpen(true)} size="sm">
                  <Plus className="mr-2 h-4 w-4" />
                  添加账号
                </Button>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2">
              <Button onClick={handleToggleSelectCurrentPage} size="sm" variant="outline" disabled={currentPageIds.length === 0}>
                {isCurrentPageAllSelected ? '取消当前页' : '全选当前页'}
              </Button>
              <Button onClick={deselectAll} size="sm" variant="outline" disabled={selectedIds.size === 0}>清空已选</Button>
              <Button onClick={handleBatchVerify} size="sm" variant="outline" disabled={selectedIds.size === 0}>
                <CheckCircle2 className="mr-2 h-4 w-4" />
                批量验活
              </Button>
              <Button onClick={handleBatchForceRefresh} size="sm" variant="outline" disabled={selectedIds.size === 0 || batchRefreshing}>
                <RefreshCw className={cn('mr-2 h-4 w-4', batchRefreshing && 'animate-spin')} />
                {batchRefreshing ? `刷新 ${batchRefreshProgress.current}/${batchRefreshProgress.total}` : '批量刷新 Token'}
              </Button>
              <Button onClick={handleBatchResetFailure} size="sm" variant="outline" disabled={selectedIds.size === 0}>
                <RotateCcw className="mr-2 h-4 w-4" />
                恢复异常
              </Button>
              <Button onClick={handleBatchDelete} size="sm" variant="destructive" disabled={selectedIds.size === 0}>
                <Trash2 className="mr-2 h-4 w-4" />
                批量删除
              </Button>
              <Button onClick={handleQueryCurrentPageInfo} size="sm" variant="outline" disabled={queryingInfo || currentCredentials.length === 0}>
                <RefreshCw className={cn('mr-2 h-4 w-4', queryingInfo && 'animate-spin')} />
                {queryingInfo ? `查询 ${queryInfoProgress.current}/${queryInfoProgress.total}` : '查询当前页信息'}
              </Button>
              <Button
                onClick={handleClearAll}
                size="sm"
                variant="outline"
                className="text-destructive hover:text-destructive"
                disabled={disabledCredentialCount === 0}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                清除已停用
              </Button>
            </div>
          </CardHeader>

          <CardContent className="space-y-4">
            <div className="flex items-center justify-between gap-3">
              <div className="truncate text-sm text-muted-foreground">
                {credentials.length === 0 ? '暂无账号' : `显示 ${credentials.length === 0 ? 0 : startIndex + 1}-${endIndex} / ${credentials.length}`}
              </div>
              <div className="flex items-center gap-2 whitespace-nowrap">
                <label htmlFor="page-size" className="text-sm text-muted-foreground">每页显示</label>
                <select
                  id="page-size"
                  className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                  value={pageSize}
                  onChange={(e) => {
                    setPageSize(Number(e.target.value))
                    setCurrentPage(1)
                  }}
                >
                  <option value={20}>20</option>
                  <option value={50}>50</option>
                  <option value={100}>100</option>
                </select>
              </div>
            </div>

            {credentials.length === 0 ? (
              <div className="rounded-md border py-10 text-center text-muted-foreground">暂无账号</div>
            ) : (
              <div className="overflow-x-auto rounded-md border">
                <table className="min-w-[1480px] w-full border-collapse">
                  <thead>
                    <tr>
                      <TableHead className="w-12">
                        <Checkbox checked={isCurrentPageAllSelected} onCheckedChange={handleToggleSelectCurrentPage} />
                      </TableHead>
                      <TableHead>账号</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead>调度</TableHead>
                      <TableHead>并发</TableHead>
                      <TableHead>最近调用</TableHead>
                      <TableHead>限频</TableHead>
                      <TableHead>粘性</TableHead>
                      <TableHead>优先级</TableHead>
                      <TableHead>接入方式</TableHead>
                      <TableHead>调度开关</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </tr>
                  </thead>
                  <tbody>
                    {currentCredentials.map((credential: CredentialStatusItem) => (
                      <CredentialRow
                        key={credential.id}
                        credential={credential}
                        onViewBalance={handleViewBalance}
                        selected={selectedIds.has(credential.id)}
                        onToggleSelect={() => toggleSelect(credential.id)}
                        balance={balanceMap.get(credential.id) || null}
                        loadingBalance={loadingBalanceIds.has(credential.id)}
                      />
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            {totalPages > 1 ? (
              <div className="flex items-center justify-between gap-4">
                <div className="truncate text-sm text-muted-foreground">
                  第 {currentPage} / {totalPages} 页，共 {credentials.length} 个账号
                </div>
                <div className="flex items-center gap-2 whitespace-nowrap">
                  <Button variant="outline" size="sm" onClick={() => setCurrentPage((p) => Math.max(1, p - 1))} disabled={currentPage === 1}>
                    上一页
                  </Button>
                  <Button variant="outline" size="sm" onClick={() => setCurrentPage((p) => Math.min(totalPages, p + 1))} disabled={currentPage === totalPages}>
                    下一页
                  </Button>
                </div>
              </div>
            ) : null}
          </CardContent>
        </Card>
      </main>

      <BalanceDialog credentialId={selectedCredentialId} open={balanceDialogOpen} onOpenChange={setBalanceDialogOpen} />
      <AddCredentialDialog open={addDialogOpen} onOpenChange={setAddDialogOpen} />
      <BatchImportDialog open={batchImportDialogOpen} onOpenChange={setBatchImportDialogOpen} />
      <KamImportDialog open={kamImportDialogOpen} onOpenChange={setKamImportDialogOpen} />
      <BatchVerifyDialog
        open={verifyDialogOpen}
        onOpenChange={setVerifyDialogOpen}
        verifying={verifying}
        progress={verifyProgress}
        results={verifyResults}
        onCancel={handleCancelVerify}
      />

      <Dialog open={versionDialogOpen} onOpenChange={setVersionDialogOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>版本信息</DialogTitle>
            <DialogDescription className="truncate whitespace-nowrap">
              当前构建、发布渠道、最近任务和维护操作都集中在这里处理。
            </DialogDescription>
          </DialogHeader>
          {systemVersion ? (
            <div className="space-y-4 text-sm">
              <div className="grid gap-3 lg:grid-cols-[minmax(0,1.1fr)_minmax(0,0.9fr)]">
                <div className="space-y-3">
                  <div className="grid gap-3 sm:grid-cols-2">
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">当前版本</div>
                      <div className="mt-1 truncate font-medium" title={systemVersion.currentVersion}>{systemVersion.currentVersion}</div>
                    </div>
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">最新版本</div>
                      <div className="mt-1 flex items-center gap-2 overflow-hidden font-medium">
                        <span className="truncate" title={systemVersion.latestVersion}>{systemVersion.latestVersion}</span>
                        {systemVersion.updateAvailable ? <Badge variant="warning" className="whitespace-nowrap">可更新</Badge> : null}
                      </div>
                    </div>
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">构建类型</div>
                      <div className="mt-1 truncate font-medium">{buildTypeLabel(systemVersion.buildType)}</div>
                    </div>
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">部署方式</div>
                      <div className="mt-1 truncate font-medium">{deploymentModeLabel(systemVersion.deploymentMode)}</div>
                    </div>
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">发布渠道</div>
                      <div className="mt-1 truncate font-medium">{channelLabel(systemVersion.channel)}</div>
                    </div>
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">当前提交</div>
                      <div className="mt-1 truncate font-medium" title={systemVersion.currentCommit || '未记录'}>{systemVersion.currentCommit || '未记录'}</div>
                    </div>
                  </div>

                  <div className="grid gap-3 sm:grid-cols-3">
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">在线更新</div>
                      <div className="mt-1">
                        <Badge variant={systemVersion.canUpdate ? 'success' : 'outline'}>{systemVersion.canUpdate ? '可用' : '不可用'}</Badge>
                      </div>
                    </div>
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">在线回滚</div>
                      <div className="mt-1">
                        <Badge variant={systemVersion.canRollback ? 'success' : 'outline'}>{systemVersion.canRollback ? '可用' : '不可用'}</Badge>
                      </div>
                    </div>
                    <div className="rounded-md border bg-muted/20 px-3 py-3">
                      <div className="truncate text-xs text-muted-foreground">在线重启</div>
                      <div className="mt-1">
                        <Badge variant={systemVersion.canRestart ? 'success' : 'outline'}>{systemVersion.canRestart ? '可用' : '不可用'}</Badge>
                      </div>
                    </div>
                  </div>

                      <div className="rounded-md border bg-muted/20 px-3 py-3">
                        <div className="truncate text-xs text-muted-foreground">维护说明</div>
                        <div className="mt-1 truncate text-foreground" title={systemVersion.updateHint}>{systemVersion.updateHint}</div>
                  </div>

                  <div className="flex items-center gap-3 overflow-hidden text-xs text-muted-foreground">
                    <span className="truncate">检查时间 {new Date(systemVersion.checkedAt).toLocaleString()}</span>
                    {systemVersion.latestPublishedAt ? (
                      <span className="truncate">发布时间 {new Date(systemVersion.latestPublishedAt).toLocaleString()}</span>
                    ) : null}
                  </div>

                  {systemVersion.releaseNotesUrl ? (
                    <a
                      href={systemVersion.releaseNotesUrl}
                      target="_blank"
                      rel="noreferrer"
                      className="inline-flex max-w-full items-center gap-1 truncate text-primary hover:underline"
                    >
                      查看发布说明
                      <ArrowUpRight className="h-3.5 w-3.5 shrink-0" />
                    </a>
                  ) : null}
                </div>

                <div className="space-y-3 rounded-md border bg-muted/15 p-4">
                  <div className="flex items-center justify-between gap-3">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium">最近任务</div>
                      <div className="truncate text-xs text-muted-foreground">
                        {latestSystemJob ? `${operationLabel(latestSystemJob)}任务状态` : '还没有维护任务记录'}
                      </div>
                    </div>
                    <Badge variant={jobStatusMeta.variant}>{jobStatusMeta.text}</Badge>
                  </div>
                  {latestSystemJob ? (
                    <div className="space-y-3 text-sm">
                      <div className="rounded-md border bg-background px-3 py-3">
                        <div className="truncate text-xs text-muted-foreground">任务摘要</div>
                        <div className="mt-1 truncate font-medium" title={latestSystemJob.message}>{latestSystemJob.message}</div>
                      </div>
                      <div className="grid gap-3 sm:grid-cols-2">
                        <div className="rounded-md border bg-background px-3 py-3">
                          <div className="truncate text-xs text-muted-foreground">任务类型</div>
                          <div className="mt-1 truncate font-medium">{operationLabel(latestSystemJob)}</div>
                        </div>
                        <div className="rounded-md border bg-background px-3 py-3">
                          <div className="truncate text-xs text-muted-foreground">目标版本 / 备份</div>
                          <div className="mt-1 truncate font-medium" title={latestSystemJob.targetVersion || '当前实例'}>
                            {latestSystemJob.targetVersion || '当前实例'}
                          </div>
                        </div>
                        <div className="rounded-md border bg-background px-3 py-3">
                          <div className="truncate text-xs text-muted-foreground">开始时间</div>
                          <div className="mt-1 truncate font-medium">
                            {latestSystemJob.startedAt ? new Date(latestSystemJob.startedAt).toLocaleString() : '未开始'}
                          </div>
                        </div>
                        <div className="rounded-md border bg-background px-3 py-3">
                          <div className="truncate text-xs text-muted-foreground">结束时间</div>
                          <div className="mt-1 truncate font-medium">
                            {latestSystemJob.finishedAt ? new Date(latestSystemJob.finishedAt).toLocaleString() : '执行中'}
                          </div>
                        </div>
                      </div>
                    </div>
                  ) : (
                    <div className="rounded-md border bg-background px-3 py-3 text-sm text-muted-foreground">
                      这里会显示最近一次更新、回滚或重启的结果。
                    </div>
                  )}
                </div>
              </div>

              <div className="grid gap-2 sm:grid-cols-4">
                <Button variant="outline" onClick={handleCheckVersion} disabled={isCheckingVersion}>
                  <RefreshCw className={cn('h-4 w-4', isCheckingVersion && 'animate-spin')} />
                  刷新信息
                </Button>
                <Button
                  variant="outline"
                  onClick={handleStartUpdate}
                  disabled={versionActionsBusy || !systemVersion.canUpdate}
                  title={updateActionTitle(systemVersion.deploymentMode, systemVersion.canUpdate)}
                >
                  <Download className="h-4 w-4" />
                  {updateActionLabel(systemVersion.deploymentMode)}
                </Button>
                {systemVersion.deploymentMode !== 'docker' ? (
                  <Button
                    variant="outline"
                    onClick={handleStartRollback}
                    disabled={versionActionsBusy || !systemVersion.canRollback}
                    title={systemVersion.canRollback ? '回滚到最近一次备份' : '当前构建不支持在线回滚'}
                  >
                    <History className="h-4 w-4" />
                    回滚
                  </Button>
                ) : null}
                <Button
                  variant="outline"
                  onClick={handleStartRestart}
                  disabled={versionActionsBusy || !systemVersion.canRestart}
                  title={systemVersion.canRestart ? '执行重启命令' : '当前未配置在线重启'}
                >
                  <Power className="h-4 w-4" />
                  重启服务
                </Button>
              </div>
            </div>
          ) : null}
        </DialogContent>
      </Dialog>
    </div>
  )
}
