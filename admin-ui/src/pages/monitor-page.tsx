import { useState } from 'react'
import { AlertTriangle, CheckCircle2, Clock3, Database, Gauge, RefreshCw, Users } from 'lucide-react'
import { Area, AreaChart, CartesianGrid, Cell, Legend, Line, LineChart, Pie, PieChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { MetricCard } from '@/components/metric-card'
import { useCredentials, useCredentialsStream, useDiagnosticsSummary } from '@/hooks/use-credentials'
import { formatDateTimeParts, formatDuration, formatNumber, formatRelativeTime, percent } from '@/lib/format'
import { extractErrorMessage } from '@/lib/utils'

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

const MODEL_COLORS = ['#3b82f6', '#10b981', '#f59e0b', '#ef4444', '#8b5cf6', '#ec4899', '#14b8a6', '#f97316']

function formatBucketTime(value: string) {
  return formatDateTimeParts(value, {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function ChartTooltip({ active, payload, label }: { active?: boolean; payload?: Array<{ name?: string; value?: number | string; color?: string; payload?: Record<string, unknown> }>; label?: string }) {
  if (!active || !payload?.length) return null
  const row = payload[0]?.payload ?? {}
  const total = typeof row.totalRequests === 'number' ? row.totalRequests : undefined
  const averageDurationMs = typeof row.averageDurationMs === 'number' ? row.averageDurationMs : undefined
  const inputTokens = typeof row.inputTokens === 'number' ? row.inputTokens : undefined
  const outputTokens = typeof row.outputTokens === 'number' ? row.outputTokens : undefined
  const cacheReadTokens = typeof row.cacheReadTokens === 'number' ? row.cacheReadTokens : undefined
  const cacheHitRate = typeof row.cacheHitRate === 'number' ? row.cacheHitRate : undefined
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
            <span className="text-muted-foreground">总 Token</span>
            <span className="font-medium">{formatNumber((inputTokens ?? 0) + (outputTokens ?? 0))}</span>
          </div>
        ) : null}
        {cacheReadTokens !== undefined && cacheReadTokens > 0 ? (
          <div className="flex min-w-32 items-center justify-between gap-4">
            <span className="text-muted-foreground">缓存命中</span>
            <span className="font-medium">{formatNumber(cacheReadTokens)}</span>
          </div>
        ) : null}
        {cacheHitRate !== undefined ? (
          <div className="flex min-w-32 items-center justify-between gap-4">
            <span className="text-muted-foreground">命中率</span>
            <span className="font-medium">{cacheHitRate.toFixed(1)}%</span>
          </div>
        ) : null}
      </div>
    </div>
  )
}

export function MonitorPage() {
  const [timeRange, setTimeRange] = useState('24h')
  const [granularity, setGranularity] = useState('hour')
  const { data, isLoading, refetch, error: credentialsError } = useCredentials()
  useCredentialsStream()
  const summary = useDiagnosticsSummary({ since: timeRange, limit: 200 })
  const credentials = data?.credentials ?? []
  const alertAccounts = credentials
    .filter((item) => item.disabled || item.dispatchState !== 'ready' || item.recent429Count > 0 || item.recentSuspiciousCount > 0)
    .slice(0, 8)
  const totalRequests = summary.data?.totalRequests ?? 0
  const successRate = percent(summary.data?.successRequests, totalRequests)
  const health = (data?.schedulableCount ?? 0) > 0 && alertAccounts.length === 0 ? '运行正常' : '需要关注'
  const pageError = credentialsError ?? summary.error

  // 计算缓存命中率
  const totalInputTokens = (summary.data?.inputTokens ?? 0)
  const cacheReadTokens = (summary.data?.cacheReadInputTokens ?? 0)
  const cacheHitRate = totalInputTokens > 0 ? (cacheReadTokens / totalInputTokens) * 100 : 0

  // 处理时间桶数据
  const trend = (summary.data?.timeBuckets ?? []).map((bucket) => ({
    ...bucket,
    label: formatBucketTime(bucket.key),
    cacheReadTokens: bucket.cacheReadInputTokens,
    cacheHitRate: bucket.inputTokens > 0 ? (bucket.cacheReadInputTokens / bucket.inputTokens) * 100 : 0,
  }))

  // 模型分布数据
  const modelRank = summary.data?.modelRank ?? []
  const modelTotal = modelRank.reduce((sum, item) => sum + item.count, 0)
  const modelPieData = modelRank.slice(0, 8).map((item, index) => ({
    name: item.key,
    value: item.count,
    color: MODEL_COLORS[index % MODEL_COLORS.length],
  }))

  // 账号使用趋势数据（Top 8）
  const credentialPerformance = (summary.data?.credentialPerformance ?? []).slice(0, 8)
  const credentialKeys = new Set(credentialPerformance.map((item) => item.key))
  const credentialTrendMap = new Map<string, Record<string, string | number>>()
  trend.forEach((bucket) => {
    credentialTrendMap.set(bucket.key, { key: bucket.key, label: bucket.label })
  })
  ;(summary.data?.credentialTimeBuckets ?? []).forEach((bucket) => {
    const key = `#${bucket.credentialId}`
    if (!credentialKeys.has(key)) return
    const row = credentialTrendMap.get(bucket.key) ?? { key: bucket.key, label: formatBucketTime(bucket.key) }
    row[key] = bucket.totalRequests
    credentialTrendMap.set(bucket.key, row)
  })
  const accountTrendData = Array.from(credentialTrendMap.values()).sort((a, b) => String(a.key).localeCompare(String(b.key)))

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">监控</h1>
          <p className="mt-1 text-sm text-muted-foreground">查看账号可用性、请求表现和需要处理的账号。</p>
        </div>
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2">
            <span className="text-sm text-muted-foreground">时间范围:</span>
            <Select value={timeRange} onValueChange={setTimeRange}>
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="1h">近 1 小时</SelectItem>
                <SelectItem value="6h">近 6 小时</SelectItem>
                <SelectItem value="24h">近 24 小时</SelectItem>
                <SelectItem value="7d">近 7 天</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-sm text-muted-foreground">粒度:</span>
            <Select value={granularity} onValueChange={setGranularity}>
              <SelectTrigger className="w-24">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="minute">按分钟</SelectItem>
                <SelectItem value="hour">按小时</SelectItem>
                <SelectItem value="day">按天</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <Button variant="outline" onClick={() => refetch()} disabled={isLoading}>
            <RefreshCw className="h-4 w-4" />
            刷新
          </Button>
        </div>
      </div>

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-6">
        <MetricCard label="系统状态" value={health} hint={`${data?.enabledCount ?? 0} 个账号已启用`} icon={Gauge} tone={health === '运行正常' ? 'text-emerald-600' : 'text-amber-600'} />
        <MetricCard label="可用账号" value={data?.schedulableCount ?? 0} hint={`共 ${data?.total ?? 0} 个账号`} icon={CheckCircle2} tone="text-emerald-600" />
        <MetricCard label="需要处理" value={alertAccounts.length} hint="冷却、阻塞或停用" icon={AlertTriangle} tone="text-amber-600" />
        <MetricCard label="24 小时请求" value={formatNumber(totalRequests)} hint={`${successRate} 成功`} icon={Users} tone="text-sky-600" />
        <MetricCard label="平均耗时" value={formatDuration(summary.data?.averageDurationMs)} hint="最近 24 小时" icon={Clock3} tone="text-indigo-600" />
        <MetricCard label="缓存命中率" value={`${cacheHitRate.toFixed(1)}%`} hint={`节省 ${formatNumber(cacheReadTokens)} tokens`} icon={Database} tone="text-purple-600" />
      </div>

      {pageError ? (
        <Card className="rounded-md border-destructive/40">
          <CardContent className="py-6 text-sm text-destructive">
            监控数据加载失败：{extractErrorMessage(pageError)}
          </CardContent>
        </Card>
      ) : null}

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_420px]">
        <Card className="rounded-md">
          <CardHeader>
            <CardTitle className="text-base">模型分布</CardTitle>
          </CardHeader>
          <CardContent>
            {modelRank.length === 0 ? (
              <div className="rounded-md border py-12 text-center text-sm text-muted-foreground">暂无数据</div>
            ) : (
              <div className="flex items-center gap-6">
                <div className="h-48 w-48 flex-shrink-0">
                  <ResponsiveContainer width="100%" height="100%">
                    <PieChart>
                      <Pie data={modelPieData} dataKey="value" nameKey="name" cx="50%" cy="50%" innerRadius={50} outerRadius={80}>
                        {modelPieData.map((entry, index) => (
                          <Cell key={`cell-${index}`} fill={entry.color} />
                        ))}
                      </Pie>
                      <Tooltip />
                    </PieChart>
                  </ResponsiveContainer>
                </div>
                <div className="flex-1 space-y-2">
                  <div className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-2 text-sm">
                    <div className="font-medium text-muted-foreground">模型</div>
                    <div className="font-medium text-muted-foreground">请求数</div>
                    {modelRank.slice(0, 6).map((item, index) => (
                      <>
                        <div key={`name-${item.key}`} className="flex items-center gap-2 truncate">
                          <span className="h-2 w-2 flex-shrink-0 rounded-full" style={{ backgroundColor: MODEL_COLORS[index % MODEL_COLORS.length] }} />
                          <span className="truncate">{item.key}</span>
                        </div>
                        <div key={`count-${item.key}`} className="text-right tabular-nums">
                          {formatNumber(item.count)}
                          <span className="ml-2 text-xs text-muted-foreground">
                            {modelTotal > 0 ? `${((item.count / modelTotal) * 100).toFixed(1)}%` : ''}
                          </span>
                        </div>
                      </>
                    ))}
                  </div>
                </div>
              </div>
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
          <CardTitle className="text-base">Token 使用趋势</CardTitle>
        </CardHeader>
        <CardContent>
          {trend.length === 0 ? (
            <div className="rounded-md border py-10 text-center text-sm text-muted-foreground">暂无数据</div>
          ) : (
            <div className="h-64">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={trend.slice(-24)} margin={{ top: 8, right: 60, left: 0, bottom: 0 }}>
                  <defs>
                    <linearGradient id="colorInput" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="5%" stopColor="#3b82f6" stopOpacity={0.3} />
                      <stop offset="95%" stopColor="#3b82f6" stopOpacity={0} />
                    </linearGradient>
                    <linearGradient id="colorOutput" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="5%" stopColor="#06b6d4" stopOpacity={0.3} />
                      <stop offset="95%" stopColor="#06b6d4" stopOpacity={0} />
                    </linearGradient>
                    <linearGradient id="colorCacheRead" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="5%" stopColor="#10b981" stopOpacity={0.3} />
                      <stop offset="95%" stopColor="#10b981" stopOpacity={0} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" vertical={false} />
                  <XAxis dataKey="label" tick={{ fontSize: 11 }} />
                  <YAxis yAxisId="left" tick={{ fontSize: 11 }} />
                  <YAxis yAxisId="right" orientation="right" tick={{ fontSize: 11 }} domain={[0, 100]} tickFormatter={(value) => `${value}%`} />
                  <Tooltip content={<ChartTooltip />} />
                  <Legend />
                  <Area yAxisId="left" type="monotone" dataKey="inputTokens" name="输入 Token" stroke="#3b82f6" fill="url(#colorInput)" />
                  <Area yAxisId="left" type="monotone" dataKey="outputTokens" name="输出 Token" stroke="#06b6d4" fill="url(#colorOutput)" />
                  <Area yAxisId="left" type="monotone" dataKey="cacheReadTokens" name="缓存命中" stroke="#10b981" fill="url(#colorCacheRead)" />
                  <Line yAxisId="right" type="monotone" dataKey="cacheHitRate" name="命中率" stroke="#8b5cf6" strokeWidth={2} dot={false} />
                </AreaChart>
              </ResponsiveContainer>
            </div>
          )}
        </CardContent>
      </Card>

      <Card className="rounded-md">
        <CardHeader>
          <CardTitle className="text-base">账号使用趋势 (Top 8)</CardTitle>
        </CardHeader>
        <CardContent>
          {credentialPerformance.length === 0 ? (
            <div className="rounded-md border py-10 text-center text-sm text-muted-foreground">暂无数据</div>
          ) : (
            <div className="h-64">
              <ResponsiveContainer width="100%" height="100%">
                <LineChart data={accountTrendData} margin={{ top: 8, right: 16, left: 0, bottom: 0 }}>
                  <CartesianGrid strokeDasharray="3 3" vertical={false} />
                  <XAxis dataKey="label" tick={{ fontSize: 11 }} />
                  <YAxis tick={{ fontSize: 11 }} />
                  <Tooltip />
                  <Legend />
                  {credentialPerformance.map((cred, index) => (
                    <Line
                      key={cred.key}
                      type="monotone"
                      dataKey={cred.key}
                      name={cred.key}
                      stroke={MODEL_COLORS[index % MODEL_COLORS.length]}
                      strokeWidth={2}
                      dot={false}
                      connectNulls
                    />
                  ))}
                </LineChart>
              </ResponsiveContainer>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
