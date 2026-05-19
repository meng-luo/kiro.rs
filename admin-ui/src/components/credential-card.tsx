import { useEffect, useMemo, useState } from 'react'
import { toast } from 'sonner'
import {
  Activity,
  Loader2,
  PlugZap,
  RefreshCw,
  Settings2,
  ShieldAlert,
  Trash2,
  Wallet,
} from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Progress } from '@/components/ui/progress'
import {
  useDeleteCredential,
  useForceRefreshToken,
  useRecoverCredential,
  useSetDisabled,
  useSetMaxConcurrent,
  useSetPriority,
} from '@/hooks/use-credentials'
import { testCredential } from '@/api/credentials'
import type { BalanceResponse, CredentialStatusItem, CredentialTestEvent } from '@/types/api'

interface CredentialCardProps {
  credential: CredentialStatusItem
  onViewBalance: (id: number) => void
  selected: boolean
  onToggleSelect: () => void
  balance: BalanceResponse | null
  loadingBalance: boolean
}

function formatLastUsed(lastUsedAt: string | null): string {
  if (!lastUsedAt) return '从未使用'
  const date = new Date(lastUsedAt)
  const now = new Date()
  const diff = now.getTime() - date.getTime()
  if (diff < 0) return '刚刚'
  const seconds = Math.floor(diff / 1000)
  if (seconds < 60) return `${seconds} 秒前`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟前`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前`
  const days = Math.floor(hours / 24)
  return `${days} 天前`
}

function formatCooldown(ms?: number): string {
  if (!ms || ms <= 0) return '无需等待'
  const totalSeconds = Math.ceil(ms / 1000)
  const hours = Math.floor(totalSeconds / 3600)
  const minutes = Math.floor((totalSeconds % 3600) / 60)
  const seconds = totalSeconds % 60
  if (hours > 0) return `${hours} 小时 ${minutes} 分`
  if (minutes > 0) return `${minutes} 分 ${seconds} 秒`
  return `${seconds} 秒`
}

function statusMeta(credential: CredentialStatusItem): {
  text: string
  variant: 'success' | 'warning' | 'outline' | 'destructive'
  title: string
} {
  if (credential.disabled) {
    return {
      text: '已禁用',
      variant: 'destructive',
      title: '当前账号已被停用，不参与调度。',
    }
  }

  switch (credential.dispatchState) {
    case 'ready':
      return {
        text: '就绪可用',
        variant: 'success',
        title: '当前可以继续承接请求。',
      }
    case 'saturated':
      return {
        text: '并发已满',
        variant: 'warning',
        title: '当前并发已达到上限，需等待已有请求释放。',
      }
    case 'cooldown':
      return {
        text: credential.lastRateLimitKind === 'suspicious_activity' ? '冷却中（风控限频）' : '冷却中',
        variant: credential.lastRateLimitKind === 'suspicious_activity' ? 'destructive' : 'outline',
        title:
          credential.lastRateLimitKind === 'suspicious_activity'
            ? '上游返回风控限频，已自动解除粘性并进入较长冷却。'
            : '当前账号刚触发限频，冷却结束后会自动恢复。',
      }
    case 'blocked':
      return {
        text: '刷新阻塞',
        variant: 'warning',
        title: '本地刷新失败达到阈值，当前不承接新请求，可手动恢复。',
      }
    case 'disabled':
      return {
        text: '已禁用',
        variant: 'destructive',
        title: '当前账号已被停用，不参与调度。',
      }
  }
}

function limitMeta(kind?: CredentialStatusItem['lastRateLimitKind']) {
  switch (kind) {
    case 'normal_429':
      return {
        text: '普通限频',
        title: '上游返回 429，请求过快，已进入短冷却。',
        variant: 'warning' as const,
      }
    case 'suspicious_activity':
      return {
        text: '风控限频',
        title: '上游返回 suspicious activity，已解除粘性并进入长冷却。',
        variant: 'destructive' as const,
      }
    case 'refresh_429':
      return {
        text: '刷新限频',
        title: '刷新 Token 时被限频，和业务请求限频分开统计。',
        variant: 'outline' as const,
      }
    default:
      return null
  }
}

function authMethodLabel(authMethod: string | null): { text: string; title: string } | null {
  switch (authMethod) {
    case 'api_key':
      return { text: 'API Key', title: '直接使用 Kiro API Key 调用。' }
    case 'idc':
      return { text: '企业登录', title: '通过企业身份登录获取访问权限。' }
    case 'social':
      return { text: '社交登录', title: '通过个人登录获取访问权限。' }
    default:
      return authMethod ? { text: authMethod, title: authMethod } : null
  }
}

function disabledReasonLabel(reason?: string) {
  switch (reason) {
    case 'Manual':
      return { text: '手动停用', title: '这是手动关闭的账号。' }
    case 'TooManyFailures':
      return { text: '连续失败过多', title: '该账号连续失败次数过多，已暂停参与调度。' }
    case 'TooManyRefreshFailures':
      return { text: '刷新失败过多', title: '该账号刷新访问状态连续失败，需人工处理。' }
    case 'QuotaExceeded':
      return { text: '额度已用尽', title: '该账号本周期可用额度已耗尽。' }
    case 'InvalidRefreshToken':
      return { text: '登录已失效', title: '该账号的刷新凭据已失效，需要重新接入。' }
    case 'InvalidConfig':
      return { text: '配置无效', title: '该账号配置不完整或格式不正确，当前无法启用。' }
    default:
      return reason ? { text: reason, title: reason } : null
  }
}

function modelOptionsFor(credential: CredentialStatusItem) {
  const supportsOpus = balanceLabel(credential).supportsOpus
  const base = [
    { value: 'claude-haiku-4.5', label: 'Claude Haiku 4.5' },
    { value: 'claude-sonnet-4.5', label: 'Claude Sonnet 4.5' },
    { value: 'claude-sonnet-4.6', label: 'Claude Sonnet 4.6' },
  ]
  if (supportsOpus) {
    base.unshift(
      { value: 'claude-opus-4.6', label: 'Claude Opus 4.6' },
      { value: 'claude-opus-4.7', label: 'Claude Opus 4.7' },
    )
  }
  return base
}

function balanceLabel(credential: CredentialStatusItem) {
  const text = credential.maskedApiKey ? '按量调用' : '账号订阅'
  const supportsOpus = credential.authMethod !== 'api_key'
  return { text, supportsOpus }
}

function probeStatusText(credential: CredentialStatusItem) {
  if (credential.disabled) return '已停用'
  switch (credential.dispatchState) {
    case 'ready':
      return '可直接测试'
    case 'saturated':
      return '当前繁忙'
    case 'cooldown':
      return '限频观察中'
    case 'blocked':
      return '待处理'
    case 'disabled':
      return '已停用'
  }
}

function dispatchPathLabel(path?: CredentialStatusItem['dispatchPath']) {
  switch (path) {
    case 'preferred':
      return { text: '指定账号', title: '这次请求固定命中当前账号。' }
    case 'sticky':
      return { text: '会话粘性', title: '这次请求沿用了已有会话绑定。' }
    case 'balanced':
      return { text: '均衡分配', title: '这次请求按当前调度策略自动选中。' }
    case 'soft_fallback':
      return { text: '软回退', title: '常规可用账号不足时，临时回退到轻度限频账号。' }
    default:
      return { text: '暂无记录', title: '还没有最近一次调度路径记录。' }
  }
}

function accountStateLabel(state?: string) {
  switch (state) {
    case 'ready':
      return { text: '可直接承接', title: '开始请求时账号可直接接单。' }
    case 'saturated':
      return { text: '并发已满', title: '开始请求时账号并发已满。' }
    case 'cooldown':
      return { text: '冷却中', title: '开始请求时账号处于本地冷却。' }
    case 'blocked':
      return { text: '本地阻塞', title: '开始请求时账号处于本地阻塞。' }
    case 'disabled':
      return { text: '已停用', title: '开始请求时账号已停用。' }
    default:
      return { text: state || '未知', title: state || '开始请求时的账号状态。' }
  }
}

function terminalClass(event: CredentialTestEvent) {
  switch (event.type) {
    case 'content':
      return 'text-emerald-600'
    case 'upstream_error':
    case 'upstream_exception':
      return 'text-red-600'
    case 'context_usage':
      return 'text-blue-600'
    case 'tool_use':
      return 'text-amber-600'
    case 'test_complete':
      return event.success ? 'text-emerald-700' : 'text-red-600'
    default:
      return 'text-slate-600'
  }
}

function terminalText(event: CredentialTestEvent) {
  switch (event.type) {
    case 'test_start':
      return `开始测试，模型：${event.model ?? '-'}，命中方式：${dispatchPathLabel(event.dispatchPath).text}，起始状态：${accountStateLabel(event.accountStateAtStart).text}${event.usedSoftFallback ? '，已走软回退' : ''}`
    case 'content':
      return event.text ?? ''
    case 'tool_use':
      return `工具调用：${event.name ?? 'unknown'}${event.stop ? '（完成）' : ''}`
    case 'context_usage':
      return `上下文使用率：${Number(event.percentage ?? 0).toFixed(2)}%`
    case 'upstream_error':
      return `上游错误：${event.code ?? 'Unknown'} ${event.message ?? ''}`.trim()
    case 'upstream_exception':
      return `上游异常：${event.exceptionType ?? 'Unknown'} ${event.message ?? ''}`.trim()
    case 'test_complete':
      return event.success ? '测试完成' : (event.message ?? '测试失败')
    default:
      return ''
  }
}

export function CredentialCard({
  credential,
  onViewBalance,
  selected,
  onToggleSelect,
  balance,
  loadingBalance,
}: CredentialCardProps) {
  const [showSettingsDialog, setShowSettingsDialog] = useState(false)
  const [showDeleteDialog, setShowDeleteDialog] = useState(false)
  const [showTestDialog, setShowTestDialog] = useState(false)
  const [priorityValue, setPriorityValue] = useState(String(credential.priority))
  const [maxConcurrentValue, setMaxConcurrentValue] = useState(String(credential.maxConcurrent))
  const [testModel, setTestModel] = useState(modelOptionsFor(credential)[0]?.value ?? 'claude-sonnet-4.6')
  const [testPrompt, setTestPrompt] = useState('请回复一句简短的话，确认连接已可用。')
  const [testing, setTesting] = useState(false)
  const [testEvents, setTestEvents] = useState<CredentialTestEvent[]>([])

  const setDisabled = useSetDisabled()
  const setPriority = useSetPriority()
  const setMaxConcurrent = useSetMaxConcurrent()
  const recoverCredential = useRecoverCredential()
  const deleteCredential = useDeleteCredential()
  const forceRefresh = useForceRefreshToken()

  const status = statusMeta(credential)
  const rateLimit = limitMeta(credential.lastRateLimitKind)
  const authMethod = authMethodLabel(credential.authMethod)
  const disabledReason = disabledReasonLabel(credential.disabledReason)
  const balanceMeta = balanceLabel(credential)
  const dispatchPathMeta = dispatchPathLabel(credential.dispatchPath)
  const loadPercentage = credential.maxConcurrent > 0
    ? Math.min(100, (credential.currentConcurrent / credential.maxConcurrent) * 100)
    : 0
  const canRecover = credential.dispatchState === 'blocked'
  const canRefresh = !credential.disabled && credential.authMethod !== 'api_key'
  const canDelete = credential.disabled

  useEffect(() => {
    setPriorityValue(String(credential.priority))
    setMaxConcurrentValue(String(credential.maxConcurrent))
  }, [credential.priority, credential.maxConcurrent])

  const infoItems = useMemo(() => {
    const items = [
      {
        label: '当前并发',
        value: `${credential.currentConcurrent} / ${credential.maxConcurrent}`,
        title: '当前已占用的请求数 / 账号允许的并发上限。',
      },
      {
        label: '优先级',
        value: String(credential.priority),
        title: '数字越小越优先。',
      },
      {
        label: '粘性会话',
        value: credential.stickyDetached ? '已解除绑定' : `${credential.stickySessionCount} 个`,
        title: credential.stickyDetached
          ? '风控触发后，会话已自动切走。'
          : '当前仍保留会话绑定，用于同会话稳定命中。',
      },
      {
        label: '最近调度',
        value: dispatchPathMeta.text,
        title: dispatchPathMeta.title,
      },
      {
        label: credential.cooldownRemainingMs ? '冷却剩余' : '最近 429',
        value: credential.cooldownRemainingMs ? formatCooldown(credential.cooldownRemainingMs) : `${credential.recent429Count} 次`,
        title: credential.cooldownRemainingMs
          ? '冷却结束后会自动恢复参与调度。'
          : '最近命中的普通限频次数。',
      },
      {
        label: '最近风控',
        value: `${credential.recentSuspiciousCount} 次`,
        title: '最近命中的 suspicious activity 次数。',
      },
      {
        label: '接入类型',
        value: balanceMeta.text,
        title: '仅用于辅助理解账号类型，不决定调度。',
      },
      {
        label: '最后调用',
        value: formatLastUsed(credential.lastUsedAt),
        title: '最后一次承接请求的时间。',
      },
      {
        label: '软回退资格',
        value: credential.softFallbackEligible ? '当前允许' : '当前不参与',
        title: credential.softFallbackEligible
          ? '当常规可用账号不足时，这个账号允许进入软回退候选。'
          : '当前状态下，这个账号不会进入软回退候选。',
      },
    ]

    if (balance || loadingBalance) {
      items.push({
        label: '剩余额度',
        value: loadingBalance
          ? '查询中...'
          : balance
            ? `${balance.remaining.toFixed(2)} / ${balance.usageLimit.toFixed(2)}`
            : '未知',
        title: '仅用于辅助观察，不决定调度。',
      })
    }

    return items
  }, [balance, balanceMeta.text, credential, dispatchPathMeta.text, dispatchPathMeta.title, loadingBalance])

  const handleToggleDisabled = () => {
    setDisabled.mutate(
      { id: credential.id, disabled: !credential.disabled },
      {
        onSuccess: (res) => toast.success(res.message),
        onError: (err) => toast.error(`操作失败: ${(err as Error).message}`),
      },
    )
  }

  const handleSaveSettings = () => {
    const nextPriority = Number(priorityValue)
    const nextMaxConcurrent = Number(maxConcurrentValue)

    if (!Number.isInteger(nextPriority) || nextPriority < 0) {
      toast.error('优先级必须是非负整数')
      return
    }
    if (!Number.isInteger(nextMaxConcurrent) || nextMaxConcurrent <= 0) {
      toast.error('并发上限必须是大于 0 的整数')
      return
    }

    setPriority.mutate(
      { id: credential.id, priority: nextPriority },
      {
        onError: (err) => toast.error(`优先级更新失败: ${(err as Error).message}`),
      },
    )
    setMaxConcurrent.mutate(
      { id: credential.id, maxConcurrent: nextMaxConcurrent },
      {
        onSuccess: () => {
          toast.success('设置已保存')
          setShowSettingsDialog(false)
        },
        onError: (err) => toast.error(`并发上限更新失败: ${(err as Error).message}`),
      },
    )
  }

  const handleRecover = () => {
    recoverCredential.mutate(credential.id, {
      onSuccess: (res) => toast.success(res.message),
      onError: (err) => toast.error(`恢复失败: ${(err as Error).message}`),
    })
  }

  const handleRefreshToken = () => {
    forceRefresh.mutate(credential.id, {
      onSuccess: (res) => toast.success(res.message),
      onError: (err) => toast.error(`刷新失败: ${(err as Error).message}`),
    })
  }

  const handleDelete = () => {
    deleteCredential.mutate(credential.id, {
      onSuccess: (res) => {
        toast.success(res.message)
        setShowDeleteDialog(false)
      },
      onError: (err) => toast.error(`删除失败: ${(err as Error).message}`),
    })
  }

  const handleRunTest = async () => {
    setTesting(true)
    setTestEvents([])
    try {
      const response = await testCredential(credential.id, {
        modelId: testModel,
        prompt: testPrompt,
      })
      if (!response.ok || !response.body) {
        const text = await response.text()
        throw new Error(text || `HTTP ${response.status}`)
      }

      const reader = response.body.getReader()
      const decoder = new TextDecoder()
      let buffer = ''

      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        buffer += decoder.decode(value, { stream: true })

        const chunks = buffer.split('\n\n')
        buffer = chunks.pop() ?? ''

        for (const chunk of chunks) {
          const line = chunk
            .split('\n')
            .find((item) => item.startsWith('data:'))
          if (!line) continue
          const payload = line.slice(5).trim()
          if (!payload) continue
          try {
            const event = JSON.parse(payload) as CredentialTestEvent
            setTestEvents((prev) => [...prev, event])
          } catch {
            setTestEvents((prev) => [...prev, { type: 'upstream_error', message: payload }])
          }
        }
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : '测试失败'
      setTestEvents((prev) => [...prev, { type: 'test_complete', success: false, message }])
      toast.error(message)
    } finally {
      setTesting(false)
    }
  }

  return (
    <>
      <Card className={credential.isCurrent ? 'ring-2 ring-primary' : ''}>
        <CardHeader className="space-y-3 pb-3">
          <div className="flex items-start justify-between gap-3">
            <div className="flex min-w-0 items-start gap-3">
              <Checkbox checked={selected} onCheckedChange={onToggleSelect} className="mt-1" />
              <div className="min-w-0 space-y-2">
                <div className="flex flex-wrap items-center gap-2">
                  <CardTitle className="text-base md:text-lg">
                    {credential.email || `凭据 #${credential.id}`}
                  </CardTitle>
                  {credential.isCurrent && <Badge variant="success">当前</Badge>}
                  <Badge variant={status.variant} title={status.title}>
                    {status.text}
                  </Badge>
                  {rateLimit && (
                    <Badge variant={rateLimit.variant} title={rateLimit.title}>
                      {rateLimit.text}
                    </Badge>
                  )}
                  {disabledReason && (
                    <Badge variant="outline" title={disabledReason.title}>
                      {disabledReason.text}
                    </Badge>
                  )}
                  {authMethod && (
                    <Badge variant="secondary" title={authMethod.title}>
                      {authMethod.text}
                    </Badge>
                  )}
                  <Badge
                    variant={credential.dispatchPath === 'soft_fallback' ? 'warning' : 'outline'}
                    title={dispatchPathMeta.title}
                  >
                    {dispatchPathMeta.text}
                  </Badge>
                  <Badge variant="outline" title="该账号当前使用的接入端点。">
                    {credential.endpoint}
                  </Badge>
                </div>
                <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
                  <span title="当前已占用的请求数 / 并发上限。">
                    并发 {credential.currentConcurrent}/{credential.maxConcurrent}
                  </span>
                  <span title="当前账号最近一次被使用的时间。">
                    最近调用 {formatLastUsed(credential.lastUsedAt)}
                  </span>
                  <span title={credential.stickyDetached ? '风控触发后，会话已自动切走。' : '当前仍保留会话绑定。'}>
                    {credential.stickyDetached ? '已解除粘性' : `${credential.stickySessionCount} 个活跃会话`}
                  </span>
                  {credential.lastSoftFallbackAt && (
                    <span title="最近一次通过软回退再次接单的时间。">
                      最近软回退 {formatLastUsed(credential.lastSoftFallbackAt)}
                    </span>
                  )}
                </div>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">参与调度</span>
              <Switch
                checked={!credential.disabled}
                onCheckedChange={handleToggleDisabled}
                disabled={setDisabled.isPending}
              />
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="space-y-2">
            <div className="flex items-center justify-between text-sm">
              <span className="text-muted-foreground">并发占用</span>
              <span className="font-medium">{loadPercentage.toFixed(0)}%</span>
            </div>
            <Progress value={loadPercentage} />
          </div>

          <div className="grid grid-cols-2 gap-3 text-sm">
            {infoItems.map((item) => (
              <div key={item.label} className="rounded-md border bg-muted/30 px-3 py-2">
                <div className="text-xs text-muted-foreground" title={item.title}>
                  {item.label}
                </div>
                <div className="mt-1 font-medium">{item.value}</div>
              </div>
            ))}
          </div>

          <div className="flex flex-wrap gap-2 border-t pt-3">
            <Button size="sm" onClick={() => setShowTestDialog(true)}>
              <PlugZap className="h-4 w-4" />
              测试接入
            </Button>
            <Button size="sm" variant="outline" onClick={() => setShowSettingsDialog(true)}>
              <Settings2 className="h-4 w-4" />
              更多设置
            </Button>
            <Button size="sm" variant="outline" onClick={() => onViewBalance(credential.id)}>
              <Wallet className="h-4 w-4" />
              查看余额
            </Button>
            {canRecover && (
              <Button size="sm" variant="outline" onClick={handleRecover} disabled={recoverCredential.isPending}>
                <ShieldAlert className="h-4 w-4" />
                手动恢复
              </Button>
            )}
            {canRefresh && (
              <Button size="sm" variant="outline" onClick={handleRefreshToken} disabled={forceRefresh.isPending}>
                <RefreshCw className={`h-4 w-4 ${forceRefresh.isPending ? 'animate-spin' : ''}`} />
                刷新 Token
              </Button>
            )}
            <Button
              size="sm"
              variant="destructive"
              onClick={() => setShowDeleteDialog(true)}
              disabled={!canDelete}
              title={canDelete ? '删除该账号' : '需要先停用账号后才能删除'}
            >
              <Trash2 className="h-4 w-4" />
              删除
            </Button>
          </div>
        </CardContent>
      </Card>

      <Dialog open={showSettingsDialog} onOpenChange={setShowSettingsDialog}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>账号设置</DialogTitle>
            <DialogDescription>
              {credential.email || `凭据 #${credential.id}`} · {credential.endpoint}
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium">优先级</label>
              <Input
                type="number"
                min="0"
                value={priorityValue}
                onChange={(e) => setPriorityValue(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">数字越小越优先。</p>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">并发上限</label>
              <Input
                type="number"
                min="1"
                value={maxConcurrentValue}
                onChange={(e) => setMaxConcurrentValue(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">账号同时承接请求的最大数量。</p>
            </div>
          </div>
          <div className="space-y-3 rounded-lg border bg-muted/20 px-4 py-4">
            <div className="text-sm font-medium">当前运行状态</div>
            <div className="grid gap-3 md:grid-cols-2">
              <div className="rounded-md border bg-background px-3 py-3">
                <div className="text-xs text-muted-foreground">当前状态</div>
                <div className="mt-1 text-sm font-medium" title={status.title}>{status.text}</div>
              </div>
              <div className="rounded-md border bg-background px-3 py-3">
                <div className="text-xs text-muted-foreground">冷却剩余</div>
                <div className="mt-1 text-sm font-medium">
                  {credential.cooldownRemainingMs ? formatCooldown(credential.cooldownRemainingMs) : '无需等待'}
                </div>
              </div>
              <div className="rounded-md border bg-background px-3 py-3">
                <div className="text-xs text-muted-foreground">最近限频</div>
                <div className="mt-1 text-sm font-medium" title={rateLimit?.title}>
                  {rateLimit?.text ?? '无'}
                </div>
              </div>
              <div className="rounded-md border bg-background px-3 py-3">
                <div className="text-xs text-muted-foreground">粘性状态</div>
                <div className="mt-1 text-sm font-medium">
                  {credential.stickyDetached ? '已解除绑定' : `${credential.stickySessionCount} 个活跃会话`}
                </div>
              </div>
              <div className="rounded-md border bg-background px-3 py-3">
                <div className="text-xs text-muted-foreground" title={dispatchPathMeta.title}>最近调度路径</div>
                <div className="mt-1 text-sm font-medium">{dispatchPathMeta.text}</div>
              </div>
              <div className="rounded-md border bg-background px-3 py-3">
                <div className="text-xs text-muted-foreground">软回退资格</div>
                <div className="mt-1 text-sm font-medium">
                  {credential.softFallbackEligible ? '当前允许' : '当前不参与'}
                  {credential.lastSoftFallbackAt ? ` · 最近 ${formatLastUsed(credential.lastSoftFallbackAt)}` : ''}
                </div>
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowSettingsDialog(false)}>
              取消
            </Button>
            <Button
              onClick={handleSaveSettings}
              disabled={setPriority.isPending || setMaxConcurrent.isPending}
            >
              保存设置
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={showTestDialog} onOpenChange={setShowTestDialog}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>测试接入</DialogTitle>
            <DialogDescription>
              {credential.email || `凭据 #${credential.id}`} · {credential.endpoint}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="rounded-lg border bg-muted/20 px-4 py-4">
              <div className="flex items-start justify-between gap-4">
                <div className="flex min-w-0 items-start gap-3">
                  <div className="mt-1 rounded-md bg-primary/10 p-2 text-primary">
                    <Activity className="h-5 w-5" />
                  </div>
                  <div className="min-w-0">
                    <div className="font-medium">{credential.email || `凭据 #${credential.id}`}</div>
                    <div className="mt-1 flex flex-wrap gap-2 text-xs text-muted-foreground">
                      <span>{authMethod?.text ?? '账号接入'}</span>
                      <span>{credential.endpoint}</span>
                      <span>并发 {credential.currentConcurrent}/{credential.maxConcurrent}</span>
                      {rateLimit && <span>{rateLimit.text}</span>}
                    </div>
                  </div>
                </div>
                <Badge variant={status.variant} title={status.title}>
                  {probeStatusText(credential)}
                </Badge>
              </div>
            </div>

            <div className="grid gap-4 md:grid-cols-[minmax(0,1fr)_220px]">
              <div className="space-y-2">
                <label className="text-sm font-medium">测试模型</label>
                <select
                  className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                  value={testModel}
                  onChange={(e) => setTestModel(e.target.value)}
                >
                  {modelOptionsFor(credential).map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </div>
              <div className="rounded-md border bg-muted/20 px-3 py-3 text-sm text-muted-foreground">
                这次测试会固定命中当前账号，用来判断这个账号此刻是否真的还能调用。
              </div>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">测试提示词</label>
              <textarea
                className="min-h-24 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                value={testPrompt}
                onChange={(e) => setTestPrompt(e.target.value)}
              />
            </div>

            <div className="rounded-md border bg-slate-50 p-4 font-mono text-xs dark:bg-slate-950">
              <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
                <div className="text-[11px] text-muted-foreground">实时输出</div>
                <div className="flex flex-wrap gap-2 text-[11px] text-muted-foreground">
                  <span>测试模型：{modelOptionsFor(credential).find((item) => item.value === testModel)?.label ?? testModel}</span>
                  <span>{testPrompt.trim() ? `提示词：${testPrompt.trim()}` : '提示词：默认检查语句'}</span>
                </div>
              </div>
              <div className="max-h-72 space-y-2 overflow-y-auto">
                {testEvents.length === 0 ? (
                  <div className="text-muted-foreground">点击“开始测试”后，这里会持续显示真实流式输出。</div>
                ) : (
                  testEvents.map((event, index) => (
                    <div key={`${event.type}-${index}`} className={terminalClass(event)}>
                      {terminalText(event)}
                    </div>
                  ))
                )}
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowTestDialog(false)} disabled={testing}>
              关闭
            </Button>
            <Button onClick={handleRunTest} disabled={testing}>
              {testing ? <Loader2 className="h-4 w-4 animate-spin" /> : <PlugZap className="h-4 w-4" />}
              {testing ? '测试中...' : '开始测试'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>确认删除账号</DialogTitle>
            <DialogDescription>
              仅允许删除已停用的账号。删除后无法恢复。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowDeleteDialog(false)} disabled={deleteCredential.isPending}>
              取消
            </Button>
            <Button variant="destructive" onClick={handleDelete} disabled={deleteCredential.isPending || !canDelete}>
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
