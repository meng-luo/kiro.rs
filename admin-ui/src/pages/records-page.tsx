import { useEffect, useState } from 'react'
import { Clipboard, Copy, Download, RefreshCw } from 'lucide-react'
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

function rateLimitLabel(kind?: string | null) {
  switch (kind) {
    case 'normal_429': return '普通限频'
    case 'suspicious_activity': return '风控限频'
    case 'refresh_429': return '刷新限频'
    default: return kind || '无'
  }
}

function dispatchLabel(path?: string | null) {
  switch (path) {
    case 'sticky': return '会话粘性'
    case 'balanced': return '均衡分配'
    case 'soft_fallback': return '备用账号'
    case 'preferred': return '指定账号'
    default: return path || '-'
  }
}

function copyText(text: string, hint = '已复制') {
  if (!text) return
  navigator.clipboard.writeText(text).then(() => toast.success(hint)).catch(() => toast.error('复制失败'))
}

function DetailRow({ label, value, copyable, mono }: { label: string; value?: React.ReactNode; copyable?: string; mono?: boolean }) {
  const empty = value === null || value === undefined || value === ''
  return (
    <div className="grid grid-cols-[112px_minmax(0,1fr)_auto] items-start gap-3 py-2 text-sm">
      <div className="text-muted-foreground">{label}</div>
      <div className={`${mono ? 'font-mono text-xs' : ''} ${empty ? 'text-muted-foreground' : ''} min-w-0 break-all`}>
        {empty ? '-' : value}
      </div>
      {copyable ? (
        <button
          type="button"
          onClick={() => copyText(copyable)}
          className="rounded p-1 text-muted-foreground transition hover:bg-muted hover:text-foreground"
          aria-label="复制"
        >
          <Copy className="h-3.5 w-3.5" />
        </button>
      ) : <div />}
    </div>
  )
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
    const header = ['时间', '请求 ID', '原始模型', '映射模型', '账号', '调度', '粘性命中', '结果', '限频类型', '上游状态', '错误码', '错误消息', '耗时', '输入 Token', '缓存写入 Token', '缓存命中 Token', '未命中 Token', '输出 Token']
    const rows = items.map((item) => [
      formatTime(item.startedAt),
      item.requestId,
      item.originalModel || '',
      item.mappedModel || '',
      item.credentialId ? `#${item.credentialId}` : '',
      dispatchLabel(item.dispatchPath),
      item.stickyHit ? '是' : '否',
      item.success ? '成功' : item.rateLimitKind ? '限频' : '失败',
      item.rateLimitKind ? rateLimitLabel(item.rateLimitKind) : '',
      String(item.upstreamStatus ?? ''),
      item.upstreamErrorCode || '',
      item.upstreamMessageShort || '',
      String(item.durationMs ?? ''),
      String(item.inputTokens ?? ''),
      String(item.cacheCreationInputTokens ?? ''),
      String(item.cacheReadInputTokens ?? ''),
      String(item.uncachedInputTokens ?? ''),
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
            <table className="w-full min-w-[1360px] text-sm">
              <thead className="border-b bg-muted/30">
                <tr className="text-left text-xs text-muted-foreground">
                  <th className="px-4 py-3 font-medium">时间</th>
                  <th className="px-4 py-3 font-medium">请求 ID</th>
                  <th className="px-4 py-3 font-medium">模型</th>
                  <th className="px-4 py-3 font-medium">账号</th>
                  <th className="px-4 py-3 font-medium">调度</th>
                  <th className="px-4 py-3 font-medium">结果</th>
                  <th className="px-4 py-3 font-medium">上游返回</th>
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
                    <td className="px-4 py-3">
                      <div className="max-w-[220px] truncate font-medium" title={item.originalModel ?? '-'}>
                        {item.originalModel || '-'}
                      </div>
                      <div className="mt-1 max-w-[220px] truncate text-xs text-muted-foreground" title={item.mappedModel ?? '-'}>
                        {item.mappedModel || '-'}
                      </div>
                    </td>
                    <td className="px-4 py-3"><Badge variant="outline">{item.credentialId ? `#${item.credentialId}` : '-'}</Badge></td>
                    <td className="px-4 py-3">
                      <div className="flex max-w-[220px] flex-wrap gap-1">
                        <Badge variant={item.dispatchPath === 'soft_fallback' ? 'warning' : 'outline'}>{dispatchLabel(item.dispatchPath)}</Badge>
                        {item.stickyHit ? <Badge variant="success">粘性</Badge> : null}
                        {item.stickyDetached ? <Badge variant="destructive">脱粘</Badge> : null}
                      </div>
                    </td>
                    <td className="px-4 py-3">{statusBadge(item)}</td>
                    <td className="px-4 py-3">
                      <div className="max-w-[260px] truncate" title={item.upstreamMessageShort ?? ''}>
                        {item.rateLimitKind ? rateLimitLabel(item.rateLimitKind) : item.upstreamErrorCode || item.upstreamStatus || '-'}
                      </div>
                      <div className="mt-1 max-w-[260px] truncate text-xs text-muted-foreground" title={item.upstreamMessageShort ?? ''}>
                        {item.upstreamMessageShort || '-'}
                      </div>
                    </td>
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
                    <td colSpan={10} className="px-4 py-12 text-center text-muted-foreground">暂无记录</td>
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
        <DialogContent className="max-h-[85vh] max-w-3xl overflow-y-auto">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              请求详情
              {detail.data ? statusBadge(detail.data) : null}
            </DialogTitle>
          </DialogHeader>
          {detail.isLoading ? (
            <div className="rounded-md border py-10 text-center text-muted-foreground">正在加载详情</div>
          ) : detail.data ? (
            <div className="divide-y">
              <DetailRow label="请求 ID" value={detail.data.requestId} copyable={detail.data.requestId} mono />
              <DetailRow label="开始时间" value={formatTime(detail.data.startedAt)} />
              <DetailRow label="完成时间" value={formatTime(detail.data.finishedAt)} />
              <DetailRow label="耗时" value={formatDuration(detail.data.durationMs)} />
              <DetailRow label="原始模型" value={detail.data.originalModel} />
              <DetailRow label="映射模型" value={detail.data.mappedModel} />
              <DetailRow label="账号" value={detail.data.credentialId ? `#${detail.data.credentialId}` : null} />
              <DetailRow
                label="调度路径"
                value={
                  <div className="flex flex-wrap gap-1">
                    <Badge variant={detail.data.dispatchPath === 'soft_fallback' ? 'warning' : 'outline'}>
                      {dispatchLabel(detail.data.dispatchPath)}
                    </Badge>
                    {detail.data.stickyHit ? <Badge variant="success">粘性命中</Badge> : null}
                    {detail.data.stickyDetached ? <Badge variant="destructive">已脱粘</Badge> : null}
                  </div>
                }
              />
              <DetailRow label="会话哈希" value={detail.data.sessionHash} copyable={detail.data.sessionHash ?? undefined} mono />
              <DetailRow label="上游状态" value={detail.data.upstreamStatus} />
              <DetailRow label="错误码" value={detail.data.upstreamErrorCode} />
              <DetailRow label="错误消息" value={detail.data.upstreamMessageShort} copyable={detail.data.upstreamMessageShort ?? undefined} />
              <DetailRow
                label="限频类型"
                value={detail.data.rateLimitKind ? <Badge variant="warning">{rateLimitLabel(detail.data.rateLimitKind)}</Badge> : null}
              />
              <DetailRow label="冷却时长" value={detail.data.cooldownMs ? formatDuration(detail.data.cooldownMs) : null} />
              <DetailRow label="冷却结束" value={detail.data.cooldownUntil ? formatTime(detail.data.cooldownUntil) : null} />
              <DetailRow
                label="Token"
                value={
                  <div className="space-y-1">
                    <div>合计 <span className="font-medium">{formatNumber((detail.data.inputTokens ?? 0) + (detail.data.outputTokens ?? 0))}</span></div>
                    <div className="text-xs text-muted-foreground">
                      输入 {formatNumber(detail.data.inputTokens ?? 0)} ·
                      输出 {formatNumber(detail.data.outputTokens ?? 0)} ·
                      缓存命中 {formatNumber(detail.data.cacheReadInputTokens ?? 0)} ·
                      缓存写入 {formatNumber(detail.data.cacheCreationInputTokens ?? 0)} ·
                      未命中 {formatNumber(detail.data.uncachedInputTokens ?? 0)}
                    </div>
                  </div>
                }
              />
              <div className="pt-3">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => copyText(JSON.stringify(detail.data, null, 2), '已复制 JSON')}
                >
                  <Clipboard className="h-4 w-4" />
                  复制 JSON
                </Button>
              </div>
            </div>
          ) : (
            <div className="rounded-md border py-10 text-center text-muted-foreground">暂无详情</div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  )
}
