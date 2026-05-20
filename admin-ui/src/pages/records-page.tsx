import { useEffect, useState } from 'react'
import { Download, RefreshCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { useAdminSettings, useCredentials, useDiagnosticsRequest, useDiagnosticsRequests, useSetAdminSettings } from '@/hooks/use-credentials'
import { formatDuration, formatNumber, formatTime } from '@/lib/format'
import { extractErrorMessage } from '@/lib/utils'
import type { DiagnosticsFilters, RequestDiagnosticEntry } from '@/types/api'
import { toast } from 'sonner'

const ranges = [
  { label: '1 小时', value: '1h' },
  { label: '6 小时', value: '6h' },
  { label: '24 小时', value: '24h' },
  { label: '7 天', value: '168h' },
]

function pageItems(currentPage: number, totalPages: number) {
  const items = new Set<number>()
  items.add(1)
  items.add(totalPages)
  for (let page = currentPage - 1; page <= currentPage + 1; page += 1) {
    if (page >= 1 && page <= totalPages) items.add(page)
  }
  return Array.from(items).sort((a, b) => a - b)
}

function statusBadge(item: RequestDiagnosticEntry) {
  if (item.success) return <Badge variant="success">成功</Badge>
  if (item.rateLimitKind) return <Badge variant="warning">限频</Badge>
  return <Badge variant="destructive">失败</Badge>
}

export function RecordsPage() {
  const [range, setRange] = useState('24h')
  const [credentialId, setCredentialId] = useState('')
  const [model, setModel] = useState('')
  const [status, setStatus] = useState('all')
  const [cursor, setCursor] = useState<number | undefined>(undefined)
  const [pageSize, setPageSize] = useState(10)
  const [keyword, setKeyword] = useState('')
  const [detailId, setDetailId] = useState<string | null>(null)
  const credentials = useCredentials()
  const detail = useDiagnosticsRequest(detailId)
  const adminSettings = useAdminSettings()
  const setAdminSettings = useSetAdminSettings()

  const filters: DiagnosticsFilters = {
    since: range,
    limit: pageSize,
    credentialId: credentialId ? Number(credentialId) : undefined,
    model: model.trim() || undefined,
    success: status === 'success' ? true : status === 'failed' ? false : undefined,
    rateLimitOnly: status === 'limited' ? true : undefined,
    keyword: keyword.trim() || undefined,
    cursor,
  }
  const requests = useDiagnosticsRequests(filters)
  const items = requests.data?.items ?? []
  const total = requests.data?.total ?? 0
  const currentPage = Math.floor((cursor ?? 0) / pageSize) + 1
  const totalPages = Math.max(1, Math.ceil(total / pageSize))
  const start = total === 0 ? 0 : (currentPage - 1) * pageSize + 1
  const end = Math.min(currentPage * pageSize, total)

  const resetCursor = () => setCursor(undefined)

  useEffect(() => {
    const nextPageSize = adminSettings.data?.recordsPageSize
    if (nextPageSize) {
      setPageSize((current) => (current === nextPageSize ? current : nextPageSize))
      resetCursor()
    }
  }, [adminSettings.data?.recordsPageSize])

  const goToPage = (page: number) => {
    const next = Math.max(1, Math.min(totalPages, page))
    setCursor(next === 1 ? undefined : (next - 1) * pageSize)
  }

  const changePageSize = (value: number) => {
    const previous = pageSize
    setPageSize(value)
    resetCursor()
    setAdminSettings.mutate(
      { recordsPageSize: value },
      {
        onError: (error) => {
          setPageSize(previous)
          toast.error(extractErrorMessage(error))
        },
      },
    )
  }

  const exportCsv = () => {
    const header = ['时间', '请求 ID', '模型', '账号', '状态', '耗时', '输入 Token', '缓存写入 Token', '缓存命中 Token', '输出 Token']
    const rows = items.map((item) => [
      formatTime(item.startedAt),
      item.requestId,
      item.originalModel || item.mappedModel || '',
      item.credentialId ? `#${item.credentialId}` : '',
      item.success ? '成功' : item.rateLimitKind ? '限频' : '失败',
      String(item.durationMs ?? ''),
      String(item.inputTokens ?? ''),
      String(item.cacheCreationInputTokens ?? ''),
      String(item.cacheReadInputTokens ?? ''),
      String(item.outputTokens ?? ''),
    ])
    const csv = [header, ...rows].map((row) => row.map((cell) => `"${String(cell).replace(/"/g, '""')}"`).join(',')).join('\n')
    const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = `kiro-requests-${Date.now()}.csv`
    link.click()
    URL.revokeObjectURL(url)
  }

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">使用记录</h1>
          <p className="mt-1 text-sm text-muted-foreground">查看最近请求、命中账号、耗时和失败原因。</p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" onClick={() => requests.refetch()}>
            <RefreshCw className="h-4 w-4" />
            刷新
          </Button>
          <Button variant="outline" onClick={exportCsv} disabled={items.length === 0}>
            <Download className="h-4 w-4" />
            导出
          </Button>
        </div>
      </div>

      <Card className="rounded-md">
        <CardHeader>
          <CardTitle className="text-base">筛选</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-3 md:grid-cols-5">
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={range} onChange={(event) => { setRange(event.target.value); resetCursor() }}>
            {ranges.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
          </select>
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={credentialId} onChange={(event) => { setCredentialId(event.target.value); resetCursor() }}>
            <option value="">全部账号</option>
            {(credentials.data?.credentials ?? []).map((item) => (
              <option key={item.id} value={item.id}>{item.email || `账号 #${item.id}`}</option>
            ))}
          </select>
          <Input value={model} onChange={(event) => { setModel(event.target.value); resetCursor() }} placeholder="模型名称" />
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={status} onChange={(event) => { setStatus(event.target.value); resetCursor() }}>
            <option value="all">全部结果</option>
            <option value="success">成功</option>
            <option value="failed">失败</option>
            <option value="limited">限频</option>
          </select>
          <Input value={keyword} onChange={(event) => { setKeyword(event.target.value); resetCursor() }} placeholder="请求 ID 或关键词" />
        </CardContent>
      </Card>

      <Card className="rounded-md">
        <CardContent className="p-0">
          <div className="overflow-x-auto">
            <table className="w-full min-w-[1120px] text-sm">
              <thead className="border-b bg-muted/30">
                <tr className="text-left text-xs text-muted-foreground">
                  <th className="px-4 py-3 font-medium">时间</th>
                  <th className="px-4 py-3 font-medium">请求 ID</th>
                  <th className="px-4 py-3 font-medium">模型</th>
                  <th className="px-4 py-3 font-medium">账号</th>
                  <th className="px-4 py-3 font-medium">结果</th>
                  <th className="px-4 py-3 font-medium">耗时</th>
                  <th className="px-4 py-3 font-medium">Token</th>
                  <th className="px-4 py-3 font-medium">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y">
                {items.map((item) => (
                  <tr key={item.requestId} className="hover:bg-muted/20">
                    <td className="px-4 py-3 whitespace-nowrap">{formatTime(item.startedAt)}</td>
                    <td className="px-4 py-3 font-mono text-xs">{item.requestId}</td>
                    <td className="px-4 py-3">{item.originalModel || item.mappedModel || '-'}</td>
                    <td className="px-4 py-3">{item.credentialId ? `#${item.credentialId}` : '-'}</td>
                    <td className="px-4 py-3">{statusBadge(item)}</td>
                    <td className="px-4 py-3">{formatDuration(item.durationMs)}</td>
                    <td className="px-4 py-3">
                      <div>{formatNumber(item.inputTokens ?? 0)} / {formatNumber(item.outputTokens ?? 0)}</div>
                      <div className="text-xs text-muted-foreground">命中 {formatNumber(item.cacheReadInputTokens ?? 0)} · 写入 {formatNumber(item.cacheCreationInputTokens ?? 0)}</div>
                    </td>
                    <td className="px-4 py-3">
                      <Button size="sm" variant="ghost" onClick={() => setDetailId(item.requestId)}>详情</Button>
                    </td>
                  </tr>
                ))}
                {items.length === 0 ? (
                  <tr>
                    <td colSpan={8} className="px-4 py-12 text-center text-muted-foreground">暂无记录</td>
                  </tr>
                ) : null}
              </tbody>
            </table>
          </div>
        </CardContent>
      </Card>

      <div className="flex flex-col gap-3 border-t pt-4 md:flex-row md:items-center md:justify-between">
        <div className="text-sm text-muted-foreground">显示 {formatNumber(start)}-{formatNumber(end)} 条，共 {formatNumber(total)} 条</div>
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" variant="outline" disabled={currentPage === 1} onClick={() => goToPage(currentPage - 1)}>上一页</Button>
          {pageItems(currentPage, totalPages).map((page, index, pages) => (
            <span key={page} className="flex items-center gap-2">
              {index > 0 && page - pages[index - 1] > 1 ? <span className="text-sm text-muted-foreground">...</span> : null}
              <Button size="sm" variant={page === currentPage ? 'default' : 'outline'} onClick={() => goToPage(page)}>{page}</Button>
            </span>
          ))}
          <Button size="sm" variant="outline" disabled={!requests.data?.nextCursor} onClick={() => goToPage(currentPage + 1)}>下一页</Button>
        </div>
        <div className="flex items-center gap-2 text-sm">
          <span className="text-muted-foreground">每页</span>
          <select className="h-9 rounded-md border border-input bg-background px-2" value={pageSize} onChange={(event) => changePageSize(Number(event.target.value))}>
            <option value={10}>10</option>
            <option value={20}>20</option>
            <option value={50}>50</option>
            <option value={100}>100</option>
          </select>
          <span className="text-muted-foreground">条</span>
        </div>
      </div>

      <Dialog open={!!detailId} onOpenChange={(open) => { if (!open) setDetailId(null) }}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>请求详情</DialogTitle>
          </DialogHeader>
          {detail.isLoading ? (
            <div className="rounded-md border py-10 text-center text-muted-foreground">正在加载详情</div>
          ) : detail.data ? (
            <div className="grid gap-3 text-sm md:grid-cols-2">
              {[
                ['请求 ID', detail.data.requestId],
                ['开始时间', formatTime(detail.data.startedAt)],
                ['完成时间', formatTime(detail.data.finishedAt)],
                ['模型', detail.data.originalModel || detail.data.mappedModel || '-'],
                ['账号', detail.data.credentialId ? `#${detail.data.credentialId}` : '-'],
                ['结果', detail.data.success ? '成功' : detail.data.rateLimitKind ? '限频' : '失败'],
                ['耗时', formatDuration(detail.data.durationMs)],
                ['Token', `输入 ${formatNumber(detail.data.inputTokens ?? 0)} / 输出 ${formatNumber(detail.data.outputTokens ?? 0)}`],
                ['缓存', `命中 ${formatNumber(detail.data.cacheReadInputTokens ?? 0)} / 写入 ${formatNumber(detail.data.cacheCreationInputTokens ?? 0)}`],
                ['提示', detail.data.upstreamMessageShort || detail.data.rateLimitKind || '-'],
              ].map(([label, value]) => (
                <div key={label} className="rounded-md border p-3">
                  <div className="text-xs text-muted-foreground">{label}</div>
                  <div className="mt-1 break-words font-medium">{value}</div>
                </div>
              ))}
            </div>
          ) : (
            <div className="rounded-md border py-10 text-center text-muted-foreground">暂无详情</div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  )
}
