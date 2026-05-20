import { AlertTriangle, CheckCircle2, Clock3, Gauge, RefreshCw, Users } from 'lucide-react'
import { Bar, BarChart, CartesianGrid, Legend, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { MetricCard } from '@/components/metric-card'
import { useCredentials, useCredentialsStream, useDiagnosticsSummary } from '@/hooks/use-credentials'
import { formatDuration, formatNumber, formatRelativeTime, percent } from '@/lib/format'

function stateText(state: string, disabled: boolean) {
  if (disabled) return '已停用'
  switch (state) {
    case 'ready':
      return '可用'
    case 'saturated':
      return '并发已满'
    case 'cooldown':
      return '冷却中'
    case 'blocked':
      return '待处理'
    default:
      return state
  }
}

function ChartTooltip({ active, payload, label }: { active?: boolean; payload?: Array<{ name?: string; value?: number | string; color?: string; payload?: Record<string, unknown> }>; label?: string }) {
  if (!active || !payload?.length) return null
  const row = payload[0]?.payload ?? {}
  const total = typeof row.totalRequests === 'number' ? row.totalRequests : undefined
  const averageDurationMs = typeof row.averageDurationMs === 'number' ? row.averageDurationMs : undefined
  const inputTokens = typeof row.inputTokens === 'number' ? row.inputTokens : undefined
  const outputTokens = typeof row.outputTokens === 'number' ? row.outputTokens : undefined
  return (
    <div className="rounded-md border bg-popover p-3 text-xs shadow-lg">
      <div className="mb-2 font-medium">{label}</div>
      <div className="space-y-1">
        {total !== undefined ? (
          <div className="flex min-w-32 items-center justify-between gap-4">
            <span className="text-muted-foreground">总请求</span>
            <span className="font-medium">{formatNumber(total)}</span>
          </div>
        ) : null}
        {payload.map((item) => (
          <div key={item.name} className="flex min-w-32 items-center justify-between gap-4">
            <span className="flex items-center gap-2 text-muted-foreground">
              <span className="h-2 w-2 rounded-full" style={{ backgroundColor: item.color }} />
              {item.name}
            </span>
            <span className="font-medium">{typeof item.value === 'number' ? formatNumber(item.value) : item.value}</span>
          </div>
        ))}
        {averageDurationMs !== undefined ? (
          <div className="flex min-w-32 items-center justify-between gap-4">
            <span className="text-muted-foreground">平均耗时</span>
            <span className="font-medium">{formatDuration(averageDurationMs)}</span>
          </div>
        ) : null}
        {inputTokens !== undefined || outputTokens !== undefined ? (
          <div className="flex min-w-32 items-center justify-between gap-4">
            <span className="text-muted-foreground">Token</span>
            <span className="font-medium">{formatNumber((inputTokens ?? 0) + (outputTokens ?? 0))}</span>
          </div>
        ) : null}
      </div>
    </div>
  )
}

export function MonitorPage() {
  const { data, isLoading, refetch } = useCredentials()
  useCredentialsStream()
  const summary = useDiagnosticsSummary({ since: '24h', limit: 200 })
  const credentials = data?.credentials ?? []
  const alertAccounts = credentials
    .filter((item) => item.disabled || item.dispatchState !== 'ready' || item.recent429Count > 0 || item.recentSuspiciousCount > 0)
    .slice(0, 8)
  const totalRequests = summary.data?.totalRequests ?? 0
  const successRate = percent(summary.data?.successRequests, totalRequests)
  const health = (data?.schedulableCount ?? 0) > 0 && alertAccounts.length === 0 ? '运行正常' : '需要关注'
  const trend = summary.data?.timeBuckets ?? []

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">监控</h1>
          <p className="mt-1 text-sm text-muted-foreground">查看账号可用性、请求表现和需要处理的账号。</p>
        </div>
        <Button variant="outline" onClick={() => refetch()} disabled={isLoading}>
          <RefreshCw className="h-4 w-4" />
          刷新
        </Button>
      </div>

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-5">
        <MetricCard label="系统状态" value={health} hint={`${data?.enabledCount ?? 0} 个账号已启用`} icon={Gauge} tone={health === '运行正常' ? 'text-emerald-600' : 'text-amber-600'} />
        <MetricCard label="可用账号" value={data?.schedulableCount ?? 0} hint={`共 ${data?.total ?? 0} 个账号`} icon={CheckCircle2} tone="text-emerald-600" />
        <MetricCard label="需要处理" value={alertAccounts.length} hint="冷却、阻塞或停用" icon={AlertTriangle} tone="text-amber-600" />
        <MetricCard label="24 小时请求" value={formatNumber(totalRequests)} hint={`${successRate} 成功`} icon={Users} tone="text-sky-600" />
        <MetricCard label="平均耗时" value={formatDuration(summary.data?.averageDurationMs)} hint="最近 24 小时" icon={Clock3} tone="text-indigo-600" />
      </div>

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_420px]">
        <Card className="rounded-md">
          <CardHeader>
            <CardTitle className="text-base">账号状态</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {credentials.length === 0 ? (
              <div className="rounded-md border py-12 text-center text-sm text-muted-foreground">还没有账号</div>
            ) : (
              credentials.slice(0, 12).map((item) => {
                const progress = item.maxConcurrent > 0 ? (item.currentConcurrent / item.maxConcurrent) * 100 : 0
                return (
                  <div key={item.id} className="rounded-md border p-4">
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="truncate font-medium">{item.email || `账号 #${item.id}`}</div>
                        <div className="mt-1 truncate text-xs text-muted-foreground">
                          {item.endpoint} · {formatRelativeTime(item.lastUsedAt)}
                        </div>
                      </div>
                      <Badge variant={item.disabled || item.dispatchState === 'blocked' ? 'destructive' : item.dispatchState === 'ready' ? 'success' : 'warning'}>
                        {stateText(item.dispatchState, item.disabled)}
                      </Badge>
                    </div>
                    <div className="mt-4 flex items-center justify-between text-xs text-muted-foreground">
                      <span>并发 {item.currentConcurrent}/{item.maxConcurrent}</span>
                      <span>{item.proxyName ? `代理：${item.proxyName}` : item.proxyMode === 'direct' ? '直连' : '默认连接'}</span>
                    </div>
                    <Progress value={progress} className="mt-2" />
                  </div>
                )
              })
            )}
          </CardContent>
        </Card>

        <Card className="rounded-md">
          <CardHeader>
            <CardTitle className="text-base">需要处理</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {alertAccounts.length === 0 ? (
              <div className="rounded-md border py-10 text-center text-sm text-muted-foreground">当前没有需要处理的账号</div>
            ) : (
              alertAccounts.map((item) => (
                <div key={item.id} className="rounded-md border p-3">
                  <div className="flex items-center justify-between gap-3">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium">{item.email || `账号 #${item.id}`}</div>
                      <div className="mt-1 truncate text-xs text-muted-foreground">
                        {stateText(item.dispatchState, item.disabled)} · {formatRelativeTime(item.lastUsedAt)}
                      </div>
                    </div>
                    <Badge variant="outline">{item.recent429Count + item.recentSuspiciousCount} 次</Badge>
                  </div>
                </div>
              ))
            )}
          </CardContent>
        </Card>
      </div>

      <Card className="rounded-md">
        <CardHeader>
          <CardTitle className="text-base">24 小时请求趋势</CardTitle>
        </CardHeader>
        <CardContent>
          {trend.length === 0 ? (
            <div className="rounded-md border py-10 text-center text-sm text-muted-foreground">暂无请求趋势</div>
          ) : (
            <div className="h-56">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={trend.slice(-12)} margin={{ top: 8, right: 16, left: 0, bottom: 0 }}>
                  <CartesianGrid strokeDasharray="3 3" vertical={false} />
                  <XAxis dataKey="key" tick={{ fontSize: 11 }} />
                  <YAxis tick={{ fontSize: 11 }} />
                  <Tooltip content={<ChartTooltip />} />
                  <Legend />
                  <Bar dataKey="successRequests" name="成功" stackId="requests" fill="#22c55e" radius={[3, 3, 0, 0]} />
                  <Bar dataKey="failedRequests" name="失败" stackId="requests" fill="#ef4444" />
                  <Bar dataKey="rateLimitedRequests" name="限频" fill="#f59e0b" />
                </BarChart>
              </ResponsiveContainer>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
