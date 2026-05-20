import { useMemo, useState } from 'react'
import { BarChart3, CheckCircle2, Clipboard, Clock3, RefreshCw, ShieldAlert, XCircle } from 'lucide-react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { useDiagnosticsCli, useDiagnosticsRequests, useDiagnosticsSummary } from '@/hooks/use-credentials'
import { cn } from '@/lib/utils'
import type { DiagnosticsBucket, DiagnosticsFilters, RequestDiagnosticEntry } from '@/types/api'

function formatNumber(value?: number | null) {
  return new Intl.NumberFormat('zh-CN').format(value ?? 0)
}

function formatDuration(ms?: number | null) {
  if (!ms) return '0 ms'
  if (ms < 1000) return `${ms} ms`
  return `${(ms / 1000).toFixed(2)} s`
}

function formatTime(value?: string | null) {
  if (!value) return '-'
  return new Date(value).toLocaleString()
}

function rateLimitLabel(kind?: string | null) {
  switch (kind) {
    case 'normal_429':
      return '普通限频'
    case 'suspicious_activity':
      return '风控限频'
    case 'refresh_429':
      return '刷新限频'
    default:
      return kind || '无'
  }
}

function dispatchLabel(path?: string | null) {
  switch (path) {
    case 'sticky':
      return '会话粘性'
    case 'balanced':
      return '均衡分配'
    case 'soft_fallback':
      return '备用账号'
    case 'preferred':
      return '指定账号'
    default:
      return path || '-'
  }
}

function StatCard({
  icon: Icon,
  label,
  value,
  hint,
  tone,
}: {
  icon: typeof BarChart3
  label: string
  value: string
  hint: string
  tone: string
}) {
  return (
    <Card className="overflow-hidden">
      <CardContent className="flex items-center gap-4 p-4">
        <div className={cn('rounded-xl p-3', tone)}>
          <Icon className="h-5 w-5" />
        </div>
        <div className="min-w-0">
          <div className="truncate text-xs text-muted-foreground">{label}</div>
          <div className="mt-1 truncate text-2xl font-semibold">{value}</div>
          <div className="mt-1 truncate text-xs text-muted-foreground">{hint}</div>
        </div>
      </CardContent>
    </Card>
  )
}

function RankList({ title, items }: { title: string; items: DiagnosticsBucket[] }) {
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-sm">{title}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-2">
        {items.length === 0 ? (
          <div className="rounded-md border border-dashed py-6 text-center text-sm text-muted-foreground">暂无数据</div>
        ) : (
          items.map((item) => (
            <div key={item.key} className="flex items-center justify-between gap-3 rounded-md bg-muted/30 px-3 py-2">
              <span className="min-w-0 truncate text-sm" title={item.key}>{item.key}</span>
              <Badge variant="outline">{formatNumber(item.count)}</Badge>
            </div>
          ))
        )}
      </CardContent>
    </Card>
  )
}

function RequestRow({ item }: { item: RequestDiagnosticEntry }) {
  return (
    <tr className="border-b text-sm">
      <td className="max-w-[190px] px-3 py-3">
        <div className="truncate font-mono text-xs" title={item.requestId}>{item.requestId}</div>
        <div className="mt-1 truncate text-xs text-muted-foreground">{formatTime(item.startedAt)}</div>
      </td>
      <td className="max-w-[220px] px-3 py-3">
        <div className="truncate font-medium" title={item.originalModel ?? '-'}>{item.originalModel ?? '-'}</div>
        <div className="mt-1 truncate text-xs text-muted-foreground" title={item.mappedModel ?? '-'}>{item.mappedModel ?? '-'}</div>
      </td>
      <td className="px-3 py-3">
        <Badge variant="outline">{item.credentialId ? `#${item.credentialId}` : '-'}</Badge>
      </td>
      <td className="px-3 py-3">
        <div className="flex flex-wrap gap-1">
          <Badge variant={item.dispatchPath === 'soft_fallback' ? 'warning' : 'outline'}>{dispatchLabel(item.dispatchPath)}</Badge>
          {item.stickyHit ? <Badge variant="success">粘性命中</Badge> : null}
          {item.stickyDetached ? <Badge variant="destructive">已脱粘</Badge> : null}
        </div>
      </td>
      <td className="px-3 py-3">
        <Badge variant={item.success ? 'success' : 'destructive'}>{item.success ? '成功' : '失败'}</Badge>
      </td>
      <td className="max-w-[220px] px-3 py-3">
        <div className="truncate" title={item.upstreamMessageShort ?? ''}>
          {item.rateLimitKind ? rateLimitLabel(item.rateLimitKind) : item.upstreamErrorCode || item.upstreamStatus || '-'}
        </div>
        <div className="mt-1 truncate text-xs text-muted-foreground" title={item.upstreamMessageShort ?? ''}>
          {item.upstreamMessageShort ?? '-'}
        </div>
      </td>
      <td className="px-3 py-3 text-right">{formatDuration(item.durationMs)}</td>
    </tr>
  )
}

export function DiagnosticsDashboard() {
  const [since, setSince] = useState('2h')
  const [credentialId, setCredentialId] = useState('')
  const [model, setModel] = useState('')
  const [rateLimitKind, setRateLimitKind] = useState('')
  const [dispatchPath, setDispatchPath] = useState('')
  const [success, setSuccess] = useState('')

  const filters = useMemo<DiagnosticsFilters>(() => ({
    since,
    credentialId: credentialId ? Number(credentialId) : undefined,
    model: model.trim() || undefined,
    rateLimitKind: rateLimitKind || undefined,
    dispatchPath: dispatchPath || undefined,
    success: success === '' ? undefined : success === 'true',
    limit: 100,
  }), [credentialId, dispatchPath, model, rateLimitKind, since, success])

  const summary = useDiagnosticsSummary(filters)
  const requests = useDiagnosticsRequests(filters)
  const cli = useDiagnosticsCli(filters)
  const data = summary.data

  const copyCli = async () => {
    if (!cli.data?.command) return
    await navigator.clipboard.writeText(cli.data.command)
    toast.success('已复制诊断命令')
  }

  return (
    <div className="space-y-6">
      <Card className="border-slate-200 bg-gradient-to-br from-slate-950 via-slate-900 to-slate-800 text-white dark:border-slate-800">
        <CardContent className="p-6">
          <div className="flex flex-col gap-4 lg:flex-row lg:items-end lg:justify-between">
            <div>
              <div className="text-sm text-slate-300">请求统计</div>
              <div className="mt-2 text-3xl font-semibold tracking-tight">按请求结果查看账号健康</div>
              <div className="mt-2 max-w-2xl text-sm text-slate-300">
                这里展示真实请求、原始模型、账号命中和限频结果。筛选后可以直接复制命令用于 CLI 排查。
              </div>
            </div>
            <Button variant="secondary" onClick={copyCli} disabled={!cli.data?.command}>
              <Clipboard className="h-4 w-4" />
              复制 CLI
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="grid gap-3 p-4 md:grid-cols-3 lg:grid-cols-6">
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">时间范围</label>
            <Input value={since} onChange={(e) => setSince(e.target.value)} placeholder="2h" />
          </div>
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">账号 ID</label>
            <Input value={credentialId} onChange={(e) => setCredentialId(e.target.value)} placeholder="例如 1" />
          </div>
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">原始模型</label>
            <Input value={model} onChange={(e) => setModel(e.target.value)} placeholder="claude-sonnet..." />
          </div>
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">限频结果</label>
            <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={rateLimitKind} onChange={(e) => setRateLimitKind(e.target.value)}>
              <option value="">全部</option>
              <option value="normal_429">普通限频</option>
              <option value="suspicious_activity">风控限频</option>
              <option value="refresh_429">刷新限频</option>
            </select>
          </div>
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">调度方式</label>
            <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={dispatchPath} onChange={(e) => setDispatchPath(e.target.value)}>
              <option value="">全部</option>
              <option value="sticky">会话粘性</option>
              <option value="balanced">均衡分配</option>
              <option value="soft_fallback">备用账号</option>
              <option value="preferred">指定账号</option>
            </select>
          </div>
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">请求结果</label>
            <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={success} onChange={(e) => setSuccess(e.target.value)}>
              <option value="">全部</option>
              <option value="true">成功</option>
              <option value="false">失败</option>
            </select>
          </div>
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-5">
        <StatCard icon={BarChart3} label="请求数" value={formatNumber(data?.totalRequests)} hint={`${formatNumber(data?.successRequests)} 次成功`} tone="bg-blue-100 text-blue-700" />
        <StatCard icon={XCircle} label="失败数" value={formatNumber(data?.failedRequests)} hint="按上游结果统计" tone="bg-rose-100 text-rose-700" />
        <StatCard icon={ShieldAlert} label="风控限频" value={formatNumber(data?.suspiciousRequests)} hint={`${formatNumber(data?.rateLimitedRequests)} 次总限频`} tone="bg-amber-100 text-amber-700" />
        <StatCard icon={Clock3} label="平均耗时" value={formatDuration(data?.averageDurationMs)} hint="从发起到上游返回" tone="bg-emerald-100 text-emerald-700" />
        <StatCard icon={CheckCircle2} label="Token" value={formatNumber((data?.inputTokens ?? 0) + (data?.outputTokens ?? 0))} hint={`${formatNumber(data?.cacheReadInputTokens ?? 0)} 命中缓存`} tone="bg-sky-100 text-sky-700" />
      </div>

      <div className="grid gap-4 lg:grid-cols-3">
        <RankList title="原始模型排行" items={data?.modelRank ?? []} />
        <RankList title="账号命中排行" items={data?.credentialRank ?? []} />
        <RankList title="错误排行" items={data?.errorRank ?? []} />
      </div>

      <Card>
        <CardHeader className="flex flex-row items-center justify-between gap-3">
          <CardTitle className="text-base">请求明细</CardTitle>
          <Button variant="outline" size="sm" onClick={() => { summary.refetch(); requests.refetch(); cli.refetch() }}>
            <RefreshCw className="h-4 w-4" />
            刷新
          </Button>
        </CardHeader>
        <CardContent>
          {requests.isLoading ? (
            <div className="rounded-md border py-10 text-center text-muted-foreground">正在加载请求记录</div>
          ) : requests.data?.items.length ? (
            <div className="overflow-x-auto rounded-md border">
              <table className="min-w-[1180px] w-full border-collapse">
                <thead>
                  <tr>
                    <th className="bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground">请求</th>
                    <th className="bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground">模型</th>
                    <th className="bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground">账号</th>
                    <th className="bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground">调度</th>
                    <th className="bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground">结果</th>
                    <th className="bg-muted/30 px-3 py-3 text-left text-xs font-medium text-muted-foreground">上游返回</th>
                    <th className="bg-muted/30 px-3 py-3 text-right text-xs font-medium text-muted-foreground">耗时</th>
                  </tr>
                </thead>
                <tbody>
                  {requests.data.items.map((item) => <RequestRow key={item.requestId} item={item} />)}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="rounded-md border py-10 text-center text-muted-foreground">当前筛选下暂无请求记录</div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
