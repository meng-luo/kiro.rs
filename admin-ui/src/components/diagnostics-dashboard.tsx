import { useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle, BarChart3, CheckCircle2, Clipboard, Clock3, Filter,
  Gauge, RefreshCw, Search, ShieldAlert, Timer, X, XCircle, Zap,
} from 'lucide-react'
import {
  Bar, BarChart, CartesianGrid, Cell, Legend, Pie, PieChart,
  ResponsiveContainer, Tooltip, XAxis, YAxis,
} from 'recharts'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  useDiagnosticsCli, useDiagnosticsSummary,
  useCredentials,
} from '@/hooks/use-credentials'
import { cn } from '@/lib/utils'
import type {
  DiagnosticsBucket, DiagnosticsFilters, DiagnosticsPerformanceItem,
  DiagnosticsSummaryResponse, DiagnosticsTimeBucket,
} from '@/types/api'

const RESULT_COLORS = ['#22c55e', '#ef4444', '#f59e0b']
const TOKEN_COLORS = {
  uncached: '#3b82f6', cacheRead: '#14b8a6',
  cacheCreation: '#a855f7', output: '#f97316',
}
const SINCE_PRESETS: Array<[string, string]> = [
  ['1小时', '1h'], ['6小时', '6h'], ['24小时', '24h'],
  ['7天', '168h'], ['30天', '720h'],
]
const SUMMARY_LIMIT = 200

function formatNumber(value?: number | null) {
  return new Intl.NumberFormat('zh-CN').format(value ?? 0)
}

function formatDuration(ms?: number | null) {
  if (!ms) return '0 ms'
  if (ms < 1000) return `${ms} ms`
  return `${(ms / 1000).toFixed(2)} s`
}

function formatPercent(value: number, fractionDigits = 1) {
  if (!Number.isFinite(value)) return '0%'
  return `${value.toFixed(fractionDigits)}%`
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

function StatCard({
  icon: Icon, label, value, hint, tone, extra,
}: {
  icon: typeof BarChart3
  label: string
  value: string
  hint: string
  tone: string
  extra?: React.ReactNode
}) {
  return (
    <Card className="overflow-hidden">
      <CardContent className="flex items-center gap-4 p-4">
        <div className={cn('rounded-xl p-3', tone)}>
          <Icon className="h-5 w-5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs text-muted-foreground">{label}</div>
          <div className="mt-1 truncate text-2xl font-semibold">{value}</div>
          <div className="mt-1 truncate text-xs text-muted-foreground">{hint}</div>
        </div>
        {extra}
      </CardContent>
    </Card>
  )
}

function RankList({
  title, items, onPick, formatter,
}: {
  title: string
  items: DiagnosticsBucket[]
  onPick?: (key: string) => void
  formatter?: (key: string) => string
}) {
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
            <button
              key={item.key}
              type="button"
              onClick={onPick ? () => onPick(item.key) : undefined}
              className={cn(
                'flex w-full items-center justify-between gap-3 rounded-md bg-muted/30 px-3 py-2 text-left',
                onPick && 'cursor-pointer transition hover:bg-muted/60',
              )}
            >
              <span className="min-w-0 truncate text-sm" title={item.key}>
                {formatter ? formatter(item.key) : item.key}
              </span>
              <Badge variant="outline">{formatNumber(item.count)}</Badge>
            </button>
          ))
        )}
      </CardContent>
    </Card>
  )
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
          <div className="flex min-w-36 items-center justify-between gap-4">
            <span className="text-muted-foreground">总请求</span>
            <span className="font-medium">{formatNumber(total)}</span>
          </div>
        ) : null}
        {payload.map((item) => (
          <div key={item.name} className="flex min-w-36 items-center justify-between gap-4">
            <span className="flex items-center gap-2 text-muted-foreground">
              <span className="h-2 w-2 rounded-full" style={{ backgroundColor: item.color }} />
              {item.name}
            </span>
            <span className="font-medium">{typeof item.value === 'number' ? formatNumber(item.value) : item.value}</span>
          </div>
        ))}
        {averageDurationMs !== undefined ? (
          <div className="flex min-w-36 items-center justify-between gap-4">
            <span className="text-muted-foreground">平均耗时</span>
            <span className="font-medium">{formatDuration(averageDurationMs)}</span>
          </div>
        ) : null}
        {inputTokens !== undefined || outputTokens !== undefined ? (
          <div className="flex min-w-36 items-center justify-between gap-4">
            <span className="text-muted-foreground">Token</span>
            <span className="font-medium">{formatNumber((inputTokens ?? 0) + (outputTokens ?? 0))}</span>
          </div>
        ) : null}
      </div>
    </div>
  )
}

function TrendBars({ items }: { items: DiagnosticsTimeBucket[] }) {
  const data = useMemo(() => items, [items])
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">请求趋势</CardTitle>
      </CardHeader>
      <CardContent>
        {data.length === 0 ? (
          <div className="rounded-md border border-dashed py-10 text-center text-sm text-muted-foreground">暂无趋势数据</div>
        ) : (
          <div className="h-72">
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={data} margin={{ top: 8, right: 16, left: 0, bottom: 0 }}>
                <CartesianGrid strokeDasharray="3 3" vertical={false} />
                <XAxis dataKey="key" tick={{ fontSize: 11 }} interval="preserveStartEnd" />
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
  )
}

function LatencyChart({ items }: { items: DiagnosticsBucket[] }) {
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">耗时分布</CardTitle>
      </CardHeader>
      <CardContent>
        {items.length === 0 ? (
          <div className="rounded-md border border-dashed py-10 text-center text-sm text-muted-foreground">暂无耗时数据</div>
        ) : (
          <div className="h-72">
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={items} margin={{ top: 8, right: 16, left: 0, bottom: 0 }}>
                <CartesianGrid strokeDasharray="3 3" vertical={false} />
                <XAxis dataKey="key" tick={{ fontSize: 11 }} />
                <YAxis tick={{ fontSize: 11 }} />
                <Tooltip content={<ChartTooltip />} />
                <Bar dataKey="count" name="请求" fill="#3b82f6" radius={[4, 4, 0, 0]} />
              </BarChart>
            </ResponsiveContainer>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function ResultChart({ data }: { data?: DiagnosticsSummaryResponse }) {
  const resultItems = [
    { name: '成功', value: data?.successRequests ?? 0 },
    { name: '失败', value: Math.max((data?.failedRequests ?? 0) - (data?.rateLimitedRequests ?? 0), 0) },
    { name: '限频', value: data?.rateLimitedRequests ?? 0 },
  ].filter((item) => item.value > 0)

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">请求结果</CardTitle>
      </CardHeader>
      <CardContent>
        {resultItems.length === 0 ? (
          <div className="rounded-md border border-dashed py-10 text-center text-sm text-muted-foreground">暂无结果数据</div>
        ) : (
          <div className="h-72">
            <ResponsiveContainer width="100%" height="100%">
              <PieChart>
                <Pie data={resultItems} dataKey="value" nameKey="name" innerRadius={54} outerRadius={88} paddingAngle={3}>
                  {resultItems.map((item, index) => (
                    <Cell key={item.name} fill={RESULT_COLORS[index % RESULT_COLORS.length]} />
                  ))}
                </Pie>
                <Tooltip content={<ChartTooltip />} />
                <Legend />
              </PieChart>
            </ResponsiveContainer>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function TokenChart({ data }: { data?: DiagnosticsSummaryResponse }) {
  const tokenItems = [
    {
      key: 'Token',
      uncached: data?.uncachedInputTokens ?? 0,
      cacheRead: data?.cacheReadInputTokens ?? 0,
      cacheCreation: data?.cacheCreationInputTokens ?? 0,
      output: data?.outputTokens ?? 0,
    },
  ]
  const total = tokenItems[0].uncached + tokenItems[0].cacheRead + tokenItems[0].cacheCreation + tokenItems[0].output

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">Token 使用</CardTitle>
      </CardHeader>
      <CardContent>
        {total === 0 ? (
          <div className="rounded-md border border-dashed py-10 text-center text-sm text-muted-foreground">暂无 Token 数据</div>
        ) : (
          <div className="h-72">
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={tokenItems} margin={{ top: 8, right: 16, left: 0, bottom: 0 }}>
                <CartesianGrid strokeDasharray="3 3" vertical={false} />
                <XAxis dataKey="key" tick={{ fontSize: 11 }} />
                <YAxis tick={{ fontSize: 11 }} />
                <Tooltip content={<ChartTooltip />} />
                <Legend />
                <Bar dataKey="uncached" name="输入" stackId="tokens" fill={TOKEN_COLORS.uncached} radius={[3, 3, 0, 0]} />
                <Bar dataKey="cacheRead" name="缓存命中" stackId="tokens" fill={TOKEN_COLORS.cacheRead} />
                <Bar dataKey="cacheCreation" name="缓存写入" stackId="tokens" fill={TOKEN_COLORS.cacheCreation} />
                <Bar dataKey="output" name="输出" stackId="tokens" fill={TOKEN_COLORS.output} />
              </BarChart>
            </ResponsiveContainer>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function PerformanceChart({
  title, items, onPick, formatter,
}: {
  title: string
  items: DiagnosticsPerformanceItem[]
  onPick?: (key: string) => void
  formatter?: (key: string) => string
}) {
  const display = items.slice(0, 8).map((item) => ({
    ...item,
    label: formatter ? formatter(item.key) : item.key,
  }))
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">{title}</CardTitle>
      </CardHeader>
      <CardContent>
        {display.length === 0 ? (
          <div className="rounded-md border border-dashed py-10 text-center text-sm text-muted-foreground">暂无数据</div>
        ) : (
          <div className="h-72">
            <ResponsiveContainer width="100%" height="100%">
              <BarChart
                data={display}
                layout="vertical"
                margin={{ top: 8, right: 18, left: 20, bottom: 0 }}
                onClick={(state) => {
                  if (!onPick) return
                  const payload = (state as { activePayload?: Array<{ payload?: DiagnosticsPerformanceItem }> } | undefined)?.activePayload
                  const item = payload?.[0]?.payload
                  if (item?.key) onPick(item.key)
                }}
              >
                <CartesianGrid strokeDasharray="3 3" horizontal={false} />
                <XAxis type="number" tick={{ fontSize: 11 }} />
                <YAxis type="category" dataKey="label" tick={{ fontSize: 11 }} width={120} />
                <Tooltip content={<ChartTooltip />} />
                <Legend />
                <Bar dataKey="successRequests" name="成功" stackId="requests" fill="#22c55e" radius={[0, 4, 4, 0]} cursor={onPick ? 'pointer' : 'default'} />
                <Bar dataKey="failedRequests" name="失败" stackId="requests" fill="#ef4444" />
                <Bar dataKey="rateLimitedRequests" name="限频" fill="#f59e0b" />
              </BarChart>
            </ResponsiveContainer>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function PerformanceTable({
  title, items, onPick, formatter,
}: {
  title: string
  items: DiagnosticsPerformanceItem[]
  onPick?: (key: string) => void
  formatter?: (key: string) => string
}) {
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">{title}</CardTitle>
      </CardHeader>
      <CardContent>
        {items.length === 0 ? (
          <div className="rounded-md border border-dashed py-10 text-center text-sm text-muted-foreground">暂无数据</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[620px] text-sm">
              <thead className="border-b text-xs text-muted-foreground">
                <tr>
                  <th className="py-2 text-left font-medium">名称</th>
                  <th className="py-2 text-right font-medium">请求</th>
                  <th className="py-2 text-right font-medium">成功率</th>
                  <th className="py-2 text-right font-medium">平均耗时</th>
                  <th className="py-2 text-right font-medium">限频</th>
                  <th className="py-2 text-right font-medium">Token</th>
                </tr>
              </thead>
              <tbody className="divide-y">
                {items.map((item) => {
                  const successRate = item.totalRequests
                    ? (item.successRequests / item.totalRequests) * 100
                    : 0
                  return (
                    <tr
                      key={item.key}
                      className={cn(onPick && 'cursor-pointer transition hover:bg-muted/40')}
                      onClick={onPick ? () => onPick(item.key) : undefined}
                    >
                      <td className="max-w-[220px] truncate py-3" title={item.key}>
                        {formatter ? formatter(item.key) : item.key}
                      </td>
                      <td className="py-3 text-right">{formatNumber(item.totalRequests)}</td>
                      <td className={cn('py-3 text-right', successRate < 90 && successRate > 0 && 'text-amber-600 dark:text-amber-400', successRate < 70 && 'text-rose-600 dark:text-rose-400')}>
                        {item.totalRequests ? formatPercent(successRate) : '-'}
                      </td>
                      <td className="py-3 text-right">{formatDuration(item.averageDurationMs)}</td>
                      <td className="py-3 text-right">{formatNumber(item.rateLimitedRequests)}</td>
                      <td className="py-3 text-right">{formatNumber(item.inputTokens + item.outputTokens)}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

interface UiFilters {
  since: string
  until: string
  credentialId: string
  model: string
  rateLimitKind: string
  rateLimitOnly: boolean
  dispatchPath: string
  success: string
  keyword: string
}

const DEFAULT_FILTERS: UiFilters = {
  since: '24h',
  until: '',
  credentialId: '',
  model: '',
  rateLimitKind: '',
  rateLimitOnly: false,
  dispatchPath: '',
  success: '',
  keyword: '',
}

function buildFilters(ui: UiFilters, limit: number, cursor?: number): DiagnosticsFilters {
  return {
    since: ui.since || undefined,
    until: ui.until.trim() || undefined,
    credentialId: ui.credentialId ? Number(ui.credentialId) : undefined,
    model: ui.model.trim() || undefined,
    rateLimitKind: ui.rateLimitKind || undefined,
    rateLimitOnly: ui.rateLimitOnly || undefined,
    dispatchPath: ui.dispatchPath || undefined,
    success: ui.success === '' ? undefined : ui.success === 'true',
    keyword: ui.keyword.trim() || undefined,
    limit,
    cursor,
  }
}

function parseCredentialKey(key: string): number | null {
  const id = Number(key.replace(/^#/, ''))
  return Number.isFinite(id) && id > 0 ? id : null
}

function activeFilterChips(ui: UiFilters, credentialLabel: (id?: number | null) => string): Array<{ key: keyof UiFilters; label: string }> {
  const chips: Array<{ key: keyof UiFilters; label: string }> = []
  if (ui.credentialId) chips.push({ key: 'credentialId', label: credentialLabel(Number(ui.credentialId)) })
  if (ui.model) chips.push({ key: 'model', label: `模型 ${ui.model}` })
  if (ui.rateLimitKind) chips.push({ key: 'rateLimitKind', label: rateLimitLabel(ui.rateLimitKind) })
  if (ui.rateLimitOnly) chips.push({ key: 'rateLimitOnly', label: '只看限频' })
  if (ui.dispatchPath) chips.push({ key: 'dispatchPath', label: dispatchLabel(ui.dispatchPath) })
  if (ui.success !== '') chips.push({ key: 'success', label: ui.success === 'true' ? '只看成功' : '只看失败' })
  if (ui.keyword) chips.push({ key: 'keyword', label: `关键字: ${ui.keyword}` })
  if (ui.until) chips.push({ key: 'until', label: `至: ${ui.until}` })
  return chips
}

export function DiagnosticsDashboard() {
  const [ui, setUi] = useState<UiFilters>(DEFAULT_FILTERS)
  const [keywordInput, setKeywordInput] = useState('')
  const [autoRefresh, setAutoRefresh] = useState(true)

  // keyword 防抖
  useEffect(() => {
    const handle = window.setTimeout(() => {
      setUi((prev) => (prev.keyword === keywordInput.trim() ? prev : { ...prev, keyword: keywordInput.trim() }))
    }, 300)
    return () => window.clearTimeout(handle)
  }, [keywordInput])

  const summaryFilters = useMemo(() => buildFilters(ui, SUMMARY_LIMIT), [ui])

  const summary = useDiagnosticsSummary(summaryFilters)
  const cli = useDiagnosticsCli(summaryFilters)
  const credentials = useCredentials()
  const data = summary.data
  const credentialLabels = useMemo(() => {
    const map = new Map<number, string>()
    ;(credentials.data?.credentials ?? []).forEach((item) => {
      map.set(item.id, item.email || `账号 #${item.id}`)
    })
    return map
  }, [credentials.data?.credentials])
  const credentialLabel = (id?: number | null) => (id ? credentialLabels.get(id) || `账号 #${id}` : '-')
  const credentialKeyLabel = (key: string) => credentialLabel(parseCredentialKey(key))
  const pickCredentialKey = (key: string) => {
    const id = parseCredentialKey(key)
    if (id) updateFilter('credentialId', String(id))
  }

  // 自动刷新
  useEffect(() => {
    if (!autoRefresh) return
    const handle = window.setInterval(() => {
      summary.refetch()
    }, 30000)
    return () => window.clearInterval(handle)
  }, [autoRefresh, summary])

  const updateFilter = <K extends keyof UiFilters>(key: K, value: UiFilters[K]) => {
    setUi((prev) => ({ ...prev, [key]: value }))
  }

  const resetFilter = (key: keyof UiFilters) => {
    setUi((prev) => ({ ...prev, [key]: (key === 'rateLimitOnly' ? false : '') as UiFilters[typeof key] }))
    if (key === 'keyword') setKeywordInput('')
  }

  const resetAll = () => {
    setUi(DEFAULT_FILTERS)
    setKeywordInput('')
  }

  const copyCli = async () => {
    if (!cli.data?.command) return
    await navigator.clipboard.writeText(cli.data.command)
    toast.success('已复制诊断命令')
  }

  const totalRequests = data?.totalRequests ?? 0
  const successRate = totalRequests
    ? ((data?.successRequests ?? 0) / totalRequests) * 100
    : 0
  const tokenInputTotal =
    (data?.uncachedInputTokens ?? 0) +
    (data?.cacheReadInputTokens ?? 0) +
    (data?.cacheCreationInputTokens ?? 0)
  const cacheHitRate = tokenInputTotal
    ? ((data?.cacheReadInputTokens ?? 0) / tokenInputTotal) * 100
    : 0
  const rateLimitedShare = totalRequests
    ? ((data?.rateLimitedRequests ?? 0) / totalRequests) * 100
    : 0
  const suspiciousShare = totalRequests
    ? ((data?.suspiciousRequests ?? 0) / totalRequests) * 100
    : 0
  const tokenTotal = tokenInputTotal + (data?.outputTokens ?? 0)

  const chips = activeFilterChips(ui, credentialLabel)

  return (
    <div className="space-y-6">
      <Card>
        <CardContent className="flex flex-col gap-3 p-4 md:flex-row md:items-center md:justify-between">
          <div className="flex flex-1 items-center gap-3">
            <div className="relative flex-1 max-w-xl">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={keywordInput}
                onChange={(e) => setKeywordInput(e.target.value)}
                placeholder="搜索 requestId / 错误码 / 错误消息 / 模型 / 账号"
                className="pl-9"
              />
            </div>
            <Button variant="outline" size="sm" onClick={() => { summary.refetch(); cli.refetch() }}>
              <RefreshCw className={cn('h-4 w-4', summary.isFetching && 'animate-spin')} />
              刷新
            </Button>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={autoRefresh} onCheckedChange={setAutoRefresh} />
              <span className="text-muted-foreground">{autoRefresh ? '自动刷新' : '已暂停'}</span>
            </label>
            <Button variant="secondary" size="sm" onClick={copyCli} disabled={!cli.data?.command}>
              <Clipboard className="h-4 w-4" />
              复制 CLI
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="space-y-4 p-4">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-xs text-muted-foreground">时间范围</span>
            {SINCE_PRESETS.map(([label, value]) => (
              <Button key={value} size="sm" variant={ui.since === value ? 'default' : 'outline'} onClick={() => updateFilter('since', value)}>
                {label}
              </Button>
            ))}
            <Input
              value={ui.since}
              onChange={(e) => updateFilter('since', e.target.value)}
              placeholder="自定义 since (如 2h / RFC3339)"
              className="h-9 w-56"
            />
            <Input
              value={ui.until}
              onChange={(e) => updateFilter('until', e.target.value)}
              placeholder="自定义 until (留空=至今)"
              className="h-9 w-56"
            />
          </div>
          <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-5">
            <div className="space-y-1">
              <label className="text-xs text-muted-foreground">账号</label>
              <Input value={ui.credentialId} onChange={(e) => updateFilter('credentialId', e.target.value)} placeholder="例如 1" />
            </div>
            <div className="space-y-1">
              <label className="text-xs text-muted-foreground">原始模型</label>
              <Input value={ui.model} onChange={(e) => updateFilter('model', e.target.value)} placeholder="claude-sonnet..." />
            </div>
            <div className="space-y-1">
              <label className="text-xs text-muted-foreground">限频结果</label>
              <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={ui.rateLimitKind} onChange={(e) => updateFilter('rateLimitKind', e.target.value)}>
                <option value="">全部</option>
                <option value="normal_429">普通限频</option>
                <option value="suspicious_activity">风控限频</option>
                <option value="refresh_429">刷新限频</option>
              </select>
            </div>
            <div className="space-y-1">
              <label className="text-xs text-muted-foreground">调度方式</label>
              <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={ui.dispatchPath} onChange={(e) => updateFilter('dispatchPath', e.target.value)}>
                <option value="">全部</option>
                <option value="sticky">会话粘性</option>
                <option value="balanced">均衡分配</option>
                <option value="soft_fallback">备用账号</option>
                <option value="preferred">指定账号</option>
              </select>
            </div>
            <div className="space-y-1">
              <label className="text-xs text-muted-foreground">请求结果</label>
              <select className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={ui.success} onChange={(e) => updateFilter('success', e.target.value)}>
                <option value="">全部</option>
                <option value="true">成功</option>
                <option value="false">失败</option>
              </select>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={ui.rateLimitOnly} onCheckedChange={(checked) => updateFilter('rateLimitOnly', checked)} />
              <span className="text-muted-foreground">只看限频</span>
            </label>
            {chips.length > 0 ? (
              <div className="flex flex-wrap items-center gap-1">
                <Filter className="h-3.5 w-3.5 text-muted-foreground" />
                {chips.map((chip) => (
                  <Badge key={chip.key} variant="secondary" className="gap-1 pr-1">
                    {chip.label}
                    <button
                      type="button"
                      onClick={() => resetFilter(chip.key)}
                      className="rounded-full p-0.5 transition hover:bg-muted-foreground/20"
                      aria-label="移除"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </Badge>
                ))}
                <Button size="sm" variant="ghost" onClick={resetAll}>
                  清空
                </Button>
              </div>
            ) : null}
          </div>
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-5">
        <StatCard
          icon={BarChart3}
          label="请求数"
          value={formatNumber(totalRequests)}
          hint={`成功率 ${formatPercent(successRate)}`}
          tone="bg-blue-100 text-blue-700"
        />
        <StatCard
          icon={XCircle}
          label="失败 / 限频"
          value={`${formatNumber(data?.failedRequests)} / ${formatNumber(data?.rateLimitedRequests)}`}
          hint={`限频占比 ${formatPercent(rateLimitedShare)}`}
          tone="bg-rose-100 text-rose-700"
        />
        <StatCard
          icon={ShieldAlert}
          label="风控限频"
          value={formatNumber(data?.suspiciousRequests)}
          hint={`占比 ${formatPercent(suspiciousShare)}`}
          tone="bg-amber-100 text-amber-700"
        />
        <StatCard
          icon={Clock3}
          label="平均耗时"
          value={formatDuration(data?.averageDurationMs)}
          hint={`p50 ${formatDuration(data?.p50DurationMs)} · p90 ${formatDuration(data?.p90DurationMs)} · p99 ${formatDuration(data?.p99DurationMs)}`}
          tone="bg-emerald-100 text-emerald-700"
        />
        <StatCard
          icon={CheckCircle2}
          label="Token"
          value={formatNumber(tokenTotal)}
          hint={`缓存命中率 ${formatPercent(cacheHitRate)}`}
          tone="bg-sky-100 text-sky-700"
        />
      </div>

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
        <StatCard
          icon={Zap}
          label="成功率"
          value={formatPercent(successRate)}
          hint={`成功 ${formatNumber(data?.successRequests)}`}
          tone="bg-emerald-100 text-emerald-700"
        />
        <StatCard
          icon={Gauge}
          label="缓存命中率"
          value={formatPercent(cacheHitRate)}
          hint={`命中 ${formatNumber(data?.cacheReadInputTokens)} / 输入合计 ${formatNumber(tokenInputTotal)}`}
          tone="bg-teal-100 text-teal-700"
        />
        <StatCard
          icon={Timer}
          label="p90 / p99 耗时"
          value={`${formatDuration(data?.p90DurationMs)} / ${formatDuration(data?.p99DurationMs)}`}
          hint={`p50 ${formatDuration(data?.p50DurationMs)}`}
          tone="bg-indigo-100 text-indigo-700"
        />
        <StatCard
          icon={AlertTriangle}
          label="限频 / 风控占比"
          value={`${formatPercent(rateLimitedShare)} / ${formatPercent(suspiciousShare)}`}
          hint="一键切换至 '只看限频' 排查"
          tone="bg-amber-100 text-amber-700"
          extra={
            <Button size="sm" variant="ghost" onClick={() => updateFilter('rateLimitOnly', !ui.rateLimitOnly)}>
              {ui.rateLimitOnly ? '已开启' : '只看限频'}
            </Button>
          }
        />
      </div>

      <div className="grid gap-4 lg:grid-cols-3">
        <RankList
          title="原始模型排行"
          items={data?.modelRank ?? []}
          onPick={(key) => updateFilter('model', key)}
        />
        <RankList
          title="账号命中排行"
          items={data?.credentialRank ?? []}
          formatter={credentialKeyLabel}
          onPick={pickCredentialKey}
        />
        <RankList
          title="错误排行"
          items={data?.errorRank ?? []}
          formatter={(key) => rateLimitLabel(key)}
          onPick={(key) => {
            if (['normal_429', 'suspicious_activity', 'refresh_429'].includes(key)) {
              updateFilter('rateLimitKind', key)
            } else {
              updateFilter('success', 'false')
              setKeywordInput(key)
            }
          }}
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_360px]">
        <TrendBars items={data?.timeBuckets ?? []} />
        <LatencyChart items={data?.latencyBuckets ?? []} />
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <ResultChart data={data} />
        <TokenChart data={data} />
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <PerformanceChart
          title="账号性能分析"
          items={data?.credentialPerformance ?? []}
          formatter={credentialKeyLabel}
          onPick={pickCredentialKey}
        />
        <PerformanceChart
          title="模型性能对比"
          items={data?.modelPerformance ?? []}
          onPick={(key) => updateFilter('model', key)}
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <PerformanceTable
          title="账号明细"
          items={data?.credentialPerformance ?? []}
          formatter={credentialKeyLabel}
          onPick={pickCredentialKey}
        />
        <PerformanceTable
          title="模型明细"
          items={data?.modelPerformance ?? []}
          onPick={(key) => updateFilter('model', key)}
        />
      </div>

    </div>
  )
}


