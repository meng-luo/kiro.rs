import { useEffect, useMemo, useState } from 'react'
import { CheckCircle2, FileUp, Plus, RotateCcw, Trash2, Upload } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { BatchImportDialog } from '@/components/batch-import-dialog'
import { KamImportDialog } from '@/components/kam-import-dialog'
import { CredentialRow } from '@/components/credential-row'
import { MetricCard } from '@/components/metric-card'
import {
  useAdminSettings,
  useBatchDeleteCredentials,
  useBatchRefreshBalances,
  useBatchResetCredentials,
  useBatchSetDisabled,
  useBatchUpdateCredentials,
  useCredentials,
  useCredentialsStream,
  useProxies,
  useSetAdminSettings,
} from '@/hooks/use-credentials'
import { getCredentialBalance } from '@/api/credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { BalanceResponse, BatchOperationResponse, CredentialStatusItem, SchedulerPolicy } from '@/types/api'

export function AccountsPage() {
  const [selectedCredentialId, setSelectedCredentialId] = useState<number | null>(null)
  const [selectedCredentialLabel, setSelectedCredentialLabel] = useState<string | null>(null)
  const [balanceDialogOpen, setBalanceDialogOpen] = useState(false)
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [batchImportDialogOpen, setBatchImportDialogOpen] = useState(false)
  const [kamImportDialogOpen, setKamImportDialogOpen] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set())
  const [balanceMap, setBalanceMap] = useState<Map<number, BalanceResponse>>(new Map())
  const [loadingBalanceIds, setLoadingBalanceIds] = useState<Set<number>>(new Set())
  const [queryingInfo, setQueryingInfo] = useState(false)
  const [queryInfoProgress, setQueryInfoProgress] = useState({ current: 0, total: 0 })
  const [currentPage, setCurrentPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [filter, setFilter] = useState<'all' | 'alert' | 'banned' | 'rate_limited' | 'disabled'>('all')
  const [bulkEditOpen, setBulkEditOpen] = useState(false)
  const [bulkProxyMode, setBulkProxyMode] = useState('inherit')
  const [bulkProxyId, setBulkProxyId] = useState('')
  const [bulkMaxConcurrent, setBulkMaxConcurrent] = useState('')
  const [bulkSchedulerPolicy, setBulkSchedulerPolicy] = useState<'' | SchedulerPolicy>('')

  const { data, isLoading, error } = useCredentials()
  useCredentialsStream()
  const proxies = useProxies()
  const batchDelete = useBatchDeleteCredentials()
  const batchReset = useBatchResetCredentials()
  const batchRefreshBalances = useBatchRefreshBalances()
  const batchDisabled = useBatchSetDisabled()
  const batchUpdate = useBatchUpdateCredentials()
  const adminSettings = useAdminSettings()
  const setAdminSettings = useSetAdminSettings()

  const credentials = useMemo(() => {
    const list = data?.credentials ?? []
    if (filter === 'banned') return list.filter((item) => item.accountStatus === 'banned')
    if (filter === 'rate_limited') return list.filter((item) => item.accountStatus === 'rate_limited')
    if (filter === 'disabled') return list.filter((item) => item.accountStatus === 'disabled')
    if (filter === 'alert') return list.filter((item) => item.disabled || item.dispatchState !== 'ready')
    return list
  }, [data?.credentials, filter])
  const totalPages = Math.max(1, Math.ceil(credentials.length / pageSize))
  const startIndex = (currentPage - 1) * pageSize
  const endIndex = Math.min(startIndex + pageSize, credentials.length)
  const currentCredentials = credentials.slice(startIndex, endIndex)
  const currentPageIds = currentCredentials.map((credential) => credential.id)
  const isCurrentPageAllSelected = currentPageIds.length > 0 && currentPageIds.every((id) => selectedIds.has(id))
  const selectedArray = Array.from(selectedIds)

  useEffect(() => {
    if (currentPage > totalPages) setCurrentPage(totalPages)
  }, [currentPage, totalPages])

  useEffect(() => {
    const nextPageSize = adminSettings.data?.accountsPageSize
    if (nextPageSize) {
      setPageSize((current) => (current === nextPageSize ? current : nextPageSize))
      setCurrentPage(1)
    }
  }, [adminSettings.data?.accountsPageSize])

  useEffect(() => {
    const validIds = new Set((data?.credentials ?? []).map((credential) => credential.id))
    setSelectedIds((prev) => {
      const next = new Set<number>()
      prev.forEach((id) => {
        if (validIds.has(id)) next.add(id)
      })
      return next.size === prev.size ? prev : next
    })
  }, [data?.credentials])

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const toggleSelectCurrentPage = () => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (isCurrentPageAllSelected) currentPageIds.forEach((id) => next.delete(id))
      else currentPageIds.forEach((id) => next.add(id))
      return next
    })
  }

  const runBatch = async <T extends BatchOperationResponse>(action: () => Promise<T>, successText: string): Promise<T | null> => {
    if (selectedIds.size === 0) {
      toast.error('请先选择账号')
      return null
    }
    try {
      const result = await action()
      if (result.failCount === 0) toast.success(successText)
      else toast.warning(`完成：成功 ${result.successCount} 个，失败 ${result.failCount} 个`)
      setSelectedIds(new Set())
      return result
    } catch (error) {
      toast.error(extractErrorMessage(error))
      return null
    }
  }

  const handleBatchDelete = () => {
    if (!confirm(`确定要删除 ${selectedIds.size} 个账号吗？删除后无法恢复。`)) return
    runBatch(() => batchDelete.mutateAsync({ ids: selectedArray }), '已删除选中的账号')
  }

  const handleBatchRefreshBalances = async () => {
    setLoadingBalanceIds((prev) => {
      const next = new Set(prev)
      selectedArray.forEach((id) => next.add(id))
      return next
    })
    const result = await runBatch(
      () => batchRefreshBalances.mutateAsync({ ids: selectedArray }),
      '已刷新选中账号的用量',
    )
    if (result) {
      setBalanceMap((prev) => {
        const next = new Map(prev)
        result.balances.forEach((balance) => next.set(balance.id, balance))
        return next
      })
    }
    setLoadingBalanceIds((prev) => {
      const next = new Set(prev)
      selectedArray.forEach((id) => next.delete(id))
      return next
    })
  }

  const refreshSingleBalance = async (id: number) => {
    setLoadingBalanceIds((prev) => new Set(prev).add(id))
    try {
      const balance = await getCredentialBalance(id)
      setBalanceMap((prev) => new Map(prev).set(id, balance))
      toast.success('用量已刷新')
    } catch (error) {
      toast.error(extractErrorMessage(error))
    } finally {
      setLoadingBalanceIds((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }

  const queryCurrentPageInfo = async () => {
    const ids = currentCredentials.filter((item) => !item.disabled).map((item) => item.id)
    if (ids.length === 0) {
      toast.error('当前页没有可查询的账号')
      return
    }
    setQueryingInfo(true)
    setQueryInfoProgress({ current: 0, total: ids.length })
    for (let i = 0; i < ids.length; i++) {
      const id = ids[i]
      setLoadingBalanceIds((prev) => new Set(prev).add(id))
      try {
        const balance = await getCredentialBalance(id)
        setBalanceMap((prev) => new Map(prev).set(id, balance))
      } catch {
        // 单个账号失败不影响剩余查询
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
    toast.success('当前页信息已更新')
  }

  const saveBulkEdit = () => {
    const maxConcurrent = bulkMaxConcurrent.trim() ? Number(bulkMaxConcurrent) : undefined
    const schedulerPolicy = bulkSchedulerPolicy || undefined
    if (maxConcurrent !== undefined && (!Number.isInteger(maxConcurrent) || maxConcurrent <= 0)) {
      toast.error('并发上限必须是大于 0 的整数')
      return
    }
    runBatch(
      () => batchUpdate.mutateAsync({
        ids: selectedArray,
        proxyMode: bulkProxyMode,
        proxyId: bulkProxyMode === 'proxy' ? Number(bulkProxyId) : null,
        maxConcurrent,
        schedulerPolicy,
      }),
      '已更新选中的账号',
    )
    setBulkEditOpen(false)
  }

  const changePageSize = (value: number) => {
    const previous = pageSize
    setPageSize(value)
    setCurrentPage(1)
    setAdminSettings.mutate(
      { accountsPageSize: value },
      {
        onError: (error) => {
          setPageSize(previous)
          toast.error(extractErrorMessage(error))
        },
      },
    )
  }

  if (isLoading) return <div className="rounded-md border py-16 text-center text-muted-foreground">正在加载账号</div>
  if (error) return <div className="rounded-md border py-16 text-center text-destructive">{extractErrorMessage(error)}</div>

  const allCredentials = data?.credentials ?? []
  const alertCount = allCredentials.filter((item) => item.disabled || item.dispatchState !== 'ready').length
  const bannedCount = allCredentials.filter((item) => item.accountStatus === 'banned').length
  const rateLimitedCount = allCredentials.filter((item) => item.accountStatus === 'rate_limited').length
  const disabledCount = allCredentials.filter((item) => item.accountStatus === 'disabled').length

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">账号</h1>
          <p className="mt-1 text-sm text-muted-foreground">管理账号状态、批量处理和代理绑定。</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" onClick={() => setKamImportDialogOpen(true)}><FileUp className="h-4 w-4" />KAM 导入</Button>
          <Button variant="outline" onClick={() => setBatchImportDialogOpen(true)}><Upload className="h-4 w-4" />批量导入</Button>
          <Button onClick={() => setAddDialogOpen(true)}><Plus className="h-4 w-4" />添加账号</Button>
        </div>
      </div>

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-5">
        <MetricCard label="账号总数" value={data?.total ?? 0} />
        <MetricCard label="正常账号" value={allCredentials.filter((item) => item.accountStatus === 'normal').length} />
        <MetricCard label="封号" value={bannedCount} />
        <MetricCard label="限速" value={rateLimitedCount} />
        <MetricCard label="禁用" value={disabledCount} />
      </div>

      {selectedIds.size > 0 ? (
        <div className="rounded-md bg-blue-50 p-3 dark:bg-blue-950/20">
          <div className="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <span className="font-medium text-blue-900 dark:text-blue-100">已选中 {selectedIds.size} 个账号</span>
              <span className="text-blue-200">•</span>
              <button className="text-xs font-medium text-blue-700 hover:text-blue-800" onClick={toggleSelectCurrentPage}>选择当前页</button>
              <span className="text-blue-200">•</span>
              <button className="text-xs font-medium text-blue-700 hover:text-blue-800" onClick={() => setSelectedIds(new Set())}>清空选择</button>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="destructive" onClick={handleBatchDelete}><Trash2 className="h-4 w-4" />删除</Button>
              <Button size="sm" variant="outline" onClick={() => runBatch(() => batchReset.mutateAsync({ ids: selectedArray }), '已恢复选中的账号')}><RotateCcw className="h-4 w-4" />重置状态</Button>
              <Button size="sm" variant="outline" onClick={handleBatchRefreshBalances}><CheckCircle2 className="h-4 w-4" />刷新用量</Button>
              <Button size="sm" variant="outline" onClick={() => runBatch(() => batchDisabled.mutateAsync({ ids: selectedArray, disabled: false }), '已启用选中的账号')}>启用调度</Button>
              <Button size="sm" variant="outline" onClick={() => runBatch(() => batchDisabled.mutateAsync({ ids: selectedArray, disabled: true }), '已禁用选中的账号')}>禁用调度</Button>
              <Button size="sm" onClick={() => setBulkEditOpen(true)}>批量编辑</Button>
            </div>
          </div>
        </div>
      ) : null}

      <Card className="rounded-md">
        <CardHeader className="space-y-4">
          <div className="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
            <div className="min-w-0">
              <CardTitle className="text-base">账号列表</CardTitle>
              <div className="mt-1 text-sm text-muted-foreground">
                {selectedIds.size > 0 ? <Badge variant="secondary">已选 {selectedIds.size}</Badge> : `显示 ${credentials.length} 个账号`}
              </div>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="outline" onClick={toggleSelectCurrentPage}>{isCurrentPageAllSelected ? '取消当前页' : '选择当前页'}</Button>
              <Button size="sm" variant="outline" onClick={queryCurrentPageInfo} disabled={queryingInfo}>{queryingInfo ? `查询 ${queryInfoProgress.current}/${queryInfoProgress.total}` : '查询用量'}</Button>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {[
              { key: 'all', label: `全部 ${allCredentials.length}` },
              { key: 'alert', label: `需要处理 ${alertCount}` },
              { key: 'banned', label: `封号 ${bannedCount}` },
              { key: 'rate_limited', label: `限速 ${rateLimitedCount}` },
              { key: 'disabled', label: `禁用 ${disabledCount}` },
            ].map((item) => (
              <Button key={item.key} size="sm" variant={filter === item.key ? 'default' : 'outline'} onClick={() => { setFilter(item.key as typeof filter); setCurrentPage(1) }}>
                {item.label}
              </Button>
            ))}
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex items-center border-b pb-3">
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={isCurrentPageAllSelected} onCheckedChange={toggleSelectCurrentPage} />
              <span>全选当前页</span>
            </label>
          </div>
          <div className="space-y-2">
            {currentCredentials.map((credential: CredentialStatusItem) => (
              <CredentialRow
                key={credential.id}
                credential={credential}
                onViewBalance={(id, label) => { setSelectedCredentialId(id); setSelectedCredentialLabel(label); setBalanceDialogOpen(true) }}
                onRefreshBalance={refreshSingleBalance}
                selected={selectedIds.has(credential.id)}
                onToggleSelect={() => toggleSelect(credential.id)}
                balance={balanceMap.get(credential.id) || null}
                loadingBalance={loadingBalanceIds.has(credential.id)}
                variant="card"
              />
            ))}
            {currentCredentials.length === 0 ? (
              <div className="rounded-md border py-12 text-center text-sm text-muted-foreground">暂无账号</div>
            ) : null}
          </div>
          <div className="flex items-center justify-between gap-3">
            <div className="text-sm text-muted-foreground">显示 {credentials.length === 0 ? 0 : startIndex + 1}-{endIndex} / {credentials.length}</div>
            <div className="flex items-center gap-2">
              <select className="h-9 rounded-md border border-input bg-background px-3 text-sm" value={pageSize} onChange={(event) => changePageSize(Number(event.target.value))}>
                <option value={10}>10</option>
                <option value={20}>20</option>
                <option value={50}>50</option>
                <option value={100}>100</option>
              </select>
              <Button size="sm" variant="outline" onClick={() => setCurrentPage((page) => Math.max(1, page - 1))} disabled={currentPage === 1}>上一页</Button>
              <Button size="sm" variant="outline" onClick={() => setCurrentPage((page) => Math.min(totalPages, page + 1))} disabled={currentPage === totalPages}>下一页</Button>
            </div>
          </div>
        </CardContent>
      </Card>

      <BalanceDialog credentialId={selectedCredentialId} credentialLabel={selectedCredentialLabel} open={balanceDialogOpen} onOpenChange={setBalanceDialogOpen} />
      <AddCredentialDialog open={addDialogOpen} onOpenChange={setAddDialogOpen} />
      <BatchImportDialog open={batchImportDialogOpen} onOpenChange={setBatchImportDialogOpen} />
      <KamImportDialog open={kamImportDialogOpen} onOpenChange={setKamImportDialogOpen} />
      <Dialog open={bulkEditOpen} onOpenChange={setBulkEditOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>批量编辑</DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={bulkProxyMode} onChange={(event) => setBulkProxyMode(event.target.value)}>
              <option value="inherit">使用默认连接</option>
              <option value="direct">直连</option>
              <option value="proxy">选择代理</option>
            </select>
            {bulkProxyMode === 'proxy' ? (
              <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={bulkProxyId} onChange={(event) => setBulkProxyId(event.target.value)}>
                <option value="">请选择代理</option>
                {(proxies.data?.proxies ?? []).filter((item) => !item.disabled).map((item) => (
                  <option key={item.id} value={item.id}>{item.name} · {item.host}:{item.port}</option>
                ))}
              </select>
            ) : null}
            <div className="space-y-2">
              <label className="text-sm font-medium">并发上限</label>
              <Input type="number" min="1" value={bulkMaxConcurrent} onChange={(event) => setBulkMaxConcurrent(event.target.value)} placeholder="留空则不修改" />
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">请求策略</label>
              <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={bulkSchedulerPolicy} onChange={(event) => setBulkSchedulerPolicy(event.target.value as '' | SchedulerPolicy)}>
                <option value="">不修改</option>
                <option value="stable">稳定</option>
                <option value="canary">试用</option>
              </select>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setBulkEditOpen(false)}>取消</Button>
            <Button onClick={saveBulkEdit} disabled={bulkProxyMode === 'proxy' && !bulkProxyId}>保存</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
