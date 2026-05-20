import { useState } from 'react'
import { Download, RefreshCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { useCredentials, useDiagnosticsRequests } from '@/hooks/use-credentials'
import { formatDuration, formatNumber, formatTime } from '@/lib/format'
import type { DiagnosticsFilters, RequestDiagnosticEntry } from '@/types/api'

const ranges = [
  { label: '1 小时', value: '1h' },
  { label: '6 小时', value: '6h' },
  { label: '24 小时', value: '24h' },
  { label: '7 天', value: '168h' },
]

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
  const credentials = useCredentials()

  const filters: DiagnosticsFilters = {
    since: range,
    limit: 100,
    credentialId: credentialId ? Number(credentialId) : undefined,
    model: model.trim() || undefined,
    success: status === 'success' ? true : status === 'failed' ? false : undefined,
  }
  const requests = useDiagnosticsRequests(filters)
  const items = requests.data?.items ?? []

  const exportCsv = () => {
    const header = ['时间', '请求 ID', '模型', '账号', '状态', '耗时', '输入 Token', '输出 Token']
    const rows = items.map((item) => [
      formatTime(item.startedAt),
      item.requestId,
      item.originalModel || item.mappedModel || '',
      item.credentialId ? `#${item.credentialId}` : '',
      item.success ? '成功' : item.rateLimitKind ? '限频' : '失败',
      String(item.durationMs ?? ''),
      String(item.inputTokens ?? ''),
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
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={range} onChange={(event) => setRange(event.target.value)}>
            {ranges.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
          </select>
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={credentialId} onChange={(event) => setCredentialId(event.target.value)}>
            <option value="">全部账号</option>
            {(credentials.data?.credentials ?? []).map((item) => (
              <option key={item.id} value={item.id}>{item.email || `账号 #${item.id}`}</option>
            ))}
          </select>
          <Input value={model} onChange={(event) => setModel(event.target.value)} placeholder="模型名称" />
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={status} onChange={(event) => setStatus(event.target.value)}>
            <option value="all">全部结果</option>
            <option value="success">成功</option>
            <option value="failed">失败</option>
          </select>
          <div className="flex items-center text-sm text-muted-foreground">
            共 {formatNumber(requests.data?.total ?? 0)} 条
          </div>
        </CardContent>
      </Card>

      <Card className="rounded-md">
        <CardContent className="p-0">
          <div className="overflow-x-auto">
            <table className="w-full min-w-[980px] text-sm">
              <thead className="border-b bg-muted/30">
                <tr className="text-left text-xs text-muted-foreground">
                  <th className="px-4 py-3 font-medium">时间</th>
                  <th className="px-4 py-3 font-medium">请求 ID</th>
                  <th className="px-4 py-3 font-medium">模型</th>
                  <th className="px-4 py-3 font-medium">账号</th>
                  <th className="px-4 py-3 font-medium">结果</th>
                  <th className="px-4 py-3 font-medium">耗时</th>
                  <th className="px-4 py-3 font-medium">Token</th>
                  <th className="px-4 py-3 font-medium">提示</th>
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
                    <td className="px-4 py-3">{formatNumber((item.inputTokens ?? 0) + (item.outputTokens ?? 0))}</td>
                    <td className="max-w-[240px] truncate px-4 py-3 text-muted-foreground" title={item.upstreamMessageShort ?? ''}>
                      {item.upstreamMessageShort || item.rateLimitKind || '-'}
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
    </div>
  )
}
