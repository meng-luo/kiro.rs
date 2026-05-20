import { useEffect, useMemo, useRef, useState } from 'react'
import { CheckCircle2, FileUp, Plus, RefreshCw, RotateCcw, Trash2, Upload } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { BatchImportDialog } from '@/components/batch-import-dialog'
import { KamImportDialog } from '@/components/kam-import-dialog'
import { BatchVerifyDialog, type VerifyResult } from '@/components/batch-verify-dialog'
import { CredentialRow } from '@/components/credential-row'
import { MetricCard } from '@/components/metric-card'
import {
  useBatchDeleteCredentials,
  useBatchRefreshCredentials,
  useBatchResetCredentials,
  useBatchSetDisabled,
  useBatchUpdateCredentials,
  useCredentials,
  useCredentialsStream,
  useProxies,
} from '@/hooks/use-credentials'
import { getCredentialBalance } from '@/api/credentials'
import { cn, extractErrorMessage } from '@/lib/utils'
import type { BalanceResponse, CredentialStatusItem } from '@/types/api'

function TableHead({ children, className }: { children: React.ReactNode; className?: string }) {
  return <th className={cn('bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground whitespace-nowrap', className)}>{children}</th>
}

export function AccountsPage() {
  const [selectedCredentialId, setSelectedCredentialId] = useState<number | null>(null)
  const [balanceDialogOpen, setBalanceDialogOpen] = useState(false)
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
  const [currentPage, setCurrentPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [filter, setFilter] = useState<'all' | 'alert' | 'disabled'>('all')
  const [bulkEditOpen, setBulkEditOpen] = useState(false)
  const [bulkProxyMode, setBulkProxyMode] = useState('inherit')
  const [bulkProxyId, setBulkProxyId] = useState('')
  const cancelVerifyRef = useRef(false)

  const { data, isLoading, error } = useCredentials()
  useCredentialsStream()
  const proxies = useProxies()
  const batchDelete = useBatchDeleteCredentials()
  const batchReset = useBatchResetCredentials()
  const batchRefresh = useBatchRefreshCredentials()
  const batchDisabled = useBatchSetDisabled()
  const batchUpdate = useBatchUpdateCredentials()

  const credentials = useMemo(() => {
    const list = data?.credentials ?? []
    if (filter === 'disabled') return list.filter((item) => item.disabled)
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

  const runBatch = async (action: () => Promise<{ successCount: number; failCount: number }>, successText: string) => {
    if (selectedIds.size === 0) {
      toast.error('请先选择账号')
      return
    }
    try {
      const result = await action()
      if (result.failCount === 0) toast.success(successText)
      else toast.warning(`完成：成功 ${result.successCount} 个，失败 ${result.failCount} 个`)
      setSelectedIds(new Set())
    } catch (error) {
      toast.error(extractErrorMessage(error))
    }
  }

  const handleBatchDelete = () => {
    if (!confirm(`确定要删除 ${selectedIds.size} 个账号吗？删除后无法恢复。`)) return
    runBatch(() => batchDelete.mutateAsync({ ids: selectedArray }), '已删除选中的账号')
  }

  const handleBatchVerify = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择账号')
      return
    }
    setVerifying(true)
    cancelVerifyRef.current = false
    setVerifyDialogOpen(true)
    setVerifyProgress({ current: 0, total: selectedArray.length })
    setVerifyResults(new Map(selectedArray.map((id) => [id, { id, status: 'pending' as const }])))
    let successCount = 0
    for (let i = 0; i < selectedArray.length; i++) {
      if (cancelVerifyRef.current) break
      const id = selectedArray[i]
      setVerifyResults((prev) => new Map(prev).set(id, { id, status: 'verifying' }))
      try {
        const balance = await getCredentialBalance(id)
        successCount += 1
        setVerifyResults((prev) => new Map(prev).set(id, { id, status: 'success', usage: `${balance.currentUsage}/${balance.usageLimit}` }))
      } catch (error) {
        setVerifyResults((prev) => new Map(prev).set(id, { id, status: 'failed', error: extractErrorMessage(error) }))
      }
      setVerifyProgress({ current: i + 1, total: selectedArray.length })
    }
    setVerifying(false)
    if (!cancelVerifyRef.current) toast.success(`验活完成：成功 ${successCount}/${selectedArray.length}`)
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
    runBatch(
      () => batchUpdate.mutateAsync({
        ids: selectedArray,
        proxyMode: bulkProxyMode,
        proxyId: bulkProxyMode === 'proxy' ? Number(bulkProxyId) : null,
      }),
      '已更新选中的账号',
    )
    setBulkEditOpen(false)
  }

  if (isLoading) return <div className="rounded-md border py-16 text-center text-muted-foreground">正在加载账号</div>
  if (error) return <div className="rounded-md border py-16 text-center text-destructive">{extractErrorMessage(error)}</div>

  const allCredentials = data?.credentials ?? []
  const alertCount = allCredentials.filter((item) => item.disabled || item.dispatchState !== 'ready').length
  const disabledCount = allCredentials.filter((item) => item.disabled).length
  const cachedBalanceCount = allCredentials.filter((item) => item.cachedBalance).length
  const lowBalanceCount = allCredentials.filter((item) => {
    const balance = item.cachedBalance?.balance
    return balance && balance.usageLimit > 0 && balance.usagePercentage >= 80
  }).length

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
        <MetricCard label="启用账号" value={data?.enabledCount ?? 0} />
        <MetricCard label="可用账号" value={data?.schedulableCount ?? 0} />
        <MetricCard label="需要处理" value={alertCount} />
        <MetricCard label="有余额记录" value={cachedBalanceCount} hint={lowBalanceCount > 0 ? `${lowBalanceCount} 个使用较高` : undefined} />
      </div>

      {selectedIds.size > 0 ? (
        <div className="sticky top-3 z-10 rounded-md border bg-background/95 p-3 shadow-sm backdrop-blur">
          <div className="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
            <div className="text-sm font-medium">已选择 {selectedIds.size} 个账号</div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="outline" onClick={handleBatchVerify}><CheckCircle2 className="h-4 w-4" />验活</Button>
              <Button size="sm" variant="outline" onClick={() => runBatch(() => batchRefresh.mutateAsync({ ids: selectedArray }), '已刷新选中的账号')}><RefreshCw className="h-4 w-4" />刷新 Token</Button>
              <Button size="sm" variant="outline" onClick={() => runBatch(() => batchReset.mutateAsync({ ids: selectedArray }), '已恢复选中的账号')}><RotateCcw className="h-4 w-4" />恢复</Button>
              <Button size="sm" variant="outline" onClick={() => runBatch(() => batchDisabled.mutateAsync({ ids: selectedArray, disabled: false }), '已启用选中的账号')}>启用</Button>
              <Button size="sm" variant="outline" onClick={() => runBatch(() => batchDisabled.mutateAsync({ ids: selectedArray, disabled: true }), '已停用选中的账号')}>停用</Button>
              <Button size="sm" variant="outline" onClick={() => setBulkEditOpen(true)}>绑定代理</Button>
              <Button size="sm" variant="destructive" onClick={handleBatchDelete}><Trash2 className="h-4 w-4" />删除</Button>
              <Button size="sm" variant="ghost" onClick={() => setSelectedIds(new Set())}>取消选择</Button>
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
              <Button size="sm" variant="outline" onClick={queryCurrentPageInfo} disabled={queryingInfo}>{queryingInfo ? `查询 ${queryInfoProgress.current}/${queryInfoProgress.total}` : '查询余额'}</Button>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {[
              { key: 'all', label: `全部 ${allCredentials.length}` },
              { key: 'alert', label: `需要处理 ${alertCount}` },
              { key: 'disabled', label: `已停用 ${disabledCount}` },
            ].map((item) => (
              <Button key={item.key} size="sm" variant={filter === item.key ? 'default' : 'outline'} onClick={() => { setFilter(item.key as typeof filter); setCurrentPage(1) }}>
                {item.label}
              </Button>
            ))}
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="overflow-x-auto rounded-md border">
            <table className="min-w-[1180px] w-full border-collapse">
              <thead>
                <tr>
                  <TableHead className="w-12"><Checkbox checked={isCurrentPageAllSelected} onCheckedChange={toggleSelectCurrentPage} /></TableHead>
                  <TableHead>账号</TableHead>
                  <TableHead>订阅与余额</TableHead>
                  <TableHead>并发</TableHead>
                  <TableHead>最近调用</TableHead>
                  <TableHead>限频</TableHead>
                  <TableHead>粘性</TableHead>
                  <TableHead>调度</TableHead>
                  <TableHead className="text-right">操作</TableHead>
                </tr>
              </thead>
              <tbody>
                {currentCredentials.map((credential: CredentialStatusItem) => (
                  <CredentialRow
                    key={credential.id}
                    credential={credential}
                    onViewBalance={(id) => { setSelectedCredentialId(id); setBalanceDialogOpen(true) }}
                    selected={selectedIds.has(credential.id)}
                    onToggleSelect={() => toggleSelect(credential.id)}
                    balance={balanceMap.get(credential.id) || null}
                    loadingBalance={loadingBalanceIds.has(credential.id)}
                  />
                ))}
              </tbody>
            </table>
          </div>
          <div className="flex items-center justify-between gap-3">
            <div className="text-sm text-muted-foreground">显示 {credentials.length === 0 ? 0 : startIndex + 1}-{endIndex} / {credentials.length}</div>
            <div className="flex items-center gap-2">
              <select className="h-9 rounded-md border border-input bg-background px-3 text-sm" value={pageSize} onChange={(event) => { setPageSize(Number(event.target.value)); setCurrentPage(1) }}>
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

      <BalanceDialog credentialId={selectedCredentialId} open={balanceDialogOpen} onOpenChange={setBalanceDialogOpen} />
      <AddCredentialDialog open={addDialogOpen} onOpenChange={setAddDialogOpen} />
      <BatchImportDialog open={batchImportDialogOpen} onOpenChange={setBatchImportDialogOpen} />
      <KamImportDialog open={kamImportDialogOpen} onOpenChange={setKamImportDialogOpen} />
      <BatchVerifyDialog open={verifyDialogOpen} onOpenChange={setVerifyDialogOpen} verifying={verifying} progress={verifyProgress} results={verifyResults} onCancel={() => { cancelVerifyRef.current = true; setVerifying(false) }} />

      <Dialog open={bulkEditOpen} onOpenChange={setBulkEditOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>绑定代理</DialogTitle>
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
