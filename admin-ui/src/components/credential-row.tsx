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
import { useDeleteCredential, useForceRefreshToken, useRecoverCredential, useSetDisabled, useSetMaxConcurrent, useSetPriority } from '@/hooks/use-credentials'
import { testCredential } from '@/api/credentials'
import { cn } from '@/lib/utils'
import type { BalanceResponse, CredentialStatusItem, CredentialTestEvent } from '@/types/api'

interface CredentialRowProps {
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
    return { text: '已停用', variant: 'destructive', title: '当前账号已停用，不参与调度。' }
  }

  switch (credential.dispatchState) {
    case 'ready':
      return { text: '可用', variant: 'success', title: '当前可以继续承接请求。' }
    case 'saturated':
      return { text: '并发已满', variant: 'warning', title: '当前并发已达到上限。' }
    case 'cooldown':
      return {
        text: credential.lastRateLimitKind === 'suspicious_activity' ? '风控冷却' : '冷却中',
        variant: credential.lastRateLimitKind === 'suspicious_activity' ? 'destructive' : 'outline',
        title: credential.lastRateLimitKind === 'suspicious_activity'
          ? '上游返回风控限频，已进入长冷却。'
          : '当前账号刚触发限频，冷却结束后会自动恢复。'
      }
    case 'blocked':
      return { text: '本地阻塞', variant: 'warning', title: '本地刷新失败达到阈值，当前不承接新请求。' }
    case 'disabled':
      return { text: '已停用', variant: 'destructive', title: '当前账号已停用，不参与调度。' }
  }
}

function limitMeta(kind?: CredentialStatusItem['lastRateLimitKind']) {
  switch (kind) {
    case 'normal_429':
      return { text: '普通限频', title: '上游返回 429，请求过快。', variant: 'warning' as const }
    case 'suspicious_activity':
      return { text: '风控限频', title: '上游返回 suspicious activity。', variant: 'destructive' as const }
    case 'refresh_429':
      return { text: '刷新限频', title: '刷新 Token 时被限频。', variant: 'outline' as const }
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
      return { text: '连续失败过多', title: '该账号连续失败次数过多。' }
    case 'TooManyRefreshFailures':
      return { text: '刷新失败过多', title: '该账号刷新访问状态连续失败。' }
    case 'QuotaExceeded':
      return { text: '额度已用尽', title: '该账号本周期可用额度已耗尽。' }
    case 'InvalidRefreshToken':
      return { text: '登录已失效', title: '该账号的刷新凭据已失效。' }
    case 'InvalidConfig':
      return { text: '配置无效', title: '该账号配置不完整或格式不正确。' }
    default:
      return reason ? { text: reason, title: reason } : null
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

function modelOptionsFor(credential: CredentialStatusItem) {
  const supportsOpus = credential.authMethod !== 'api_key'
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
  return credential.maskedApiKey ? '按量调用' : '账号订阅'
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

function CellText({ title, children, className }: { title?: string; children: string; className?: string }) {
  return (
    <div title={title ?? children} className={cn('max-w-full truncate whitespace-nowrap', className)}>
      {children}
    </div>
  )
}

export function CredentialRow({
  credential,
  onViewBalance,
  selected,
  onToggleSelect,
  balance,
  loadingBalance,
}: CredentialRowProps) {
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
  const dispatchPathMeta = dispatchPathLabel(credential.dispatchPath)
  const canRecover = credential.dispatchState === 'blocked'
  const canRefresh = !credential.disabled && credential.authMethod !== 'api_key'
  const progressValue = credential.maxConcurrent > 0
    ? Math.min(100, (credential.currentConcurrent / credential.maxConcurrent) * 100)
    : 0

  useEffect(() => {
    setPriorityValue(String(credential.priority))
    setMaxConcurrentValue(String(credential.maxConcurrent))
  }, [credential.priority, credential.maxConcurrent])

  const infoItems = useMemo(() => {
    const items = [
      {
        label: '当前状态',
        value: status.text,
        title: status.title,
      },
      {
        label: '冷却剩余',
        value: credential.cooldownRemainingMs ? formatCooldown(credential.cooldownRemainingMs) : '无需等待',
        title: '冷却结束后会自动恢复参与调度。',
      },
      {
        label: '最近限频',
        value: rateLimit?.text ?? '无',
        title: rateLimit?.title ?? '当前没有最近限频记录。',
      },
      {
        label: '粘性状态',
        value: credential.stickyDetached ? '已解除绑定' : `${credential.stickySessionCount} 个活跃会话`,
        title: credential.stickyDetached ? '风控触发后，会话已自动切走。' : '当前仍保留会话绑定。'
      },
      {
        label: '最近调度',
        value: dispatchPathMeta.text,
        title: dispatchPathMeta.title,
      },
      {
        label: '接入类型',
        value: balanceLabel(credential),
        title: '仅用于辅助理解账号类型，不决定调度。',
      },
      {
        label: '最后调用',
        value: formatLastUsed(credential.lastUsedAt),
        title: '最后一次承接请求的时间。',
      },
      {
        label: '剩余额度',
        value: loadingBalance ? '查询中...' : balance ? `${balance.remaining.toFixed(2)} / ${balance.usageLimit.toFixed(2)}` : '未查询',
        title: '仅用于辅助观察，不决定调度。',
      },
    ]
    return items
  }, [balance, credential, dispatchPathMeta.text, dispatchPathMeta.title, loadingBalance, rateLimit?.text, rateLimit?.title, status.text, status.title])

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
          const line = chunk.split('\n').find((item) => item.startsWith('data:'))
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
      <tr className={cn(
        'border-b align-middle text-sm',
        credential.isCurrent ? 'bg-primary/5' : 'bg-background',
        selected ? 'bg-muted/20' : ''
      )}>
        <td className="w-12 px-3 py-3">
          <Checkbox checked={selected} onCheckedChange={onToggleSelect} />
        </td>
        <td className="max-w-[220px] px-3 py-3">
          <div className="min-w-0 space-y-1">
            <CellText className="font-medium" title={credential.email || `凭据 #${credential.id}`}>
              {credential.email || `凭据 #${credential.id}`}
            </CellText>
            <div className="flex items-center gap-2 overflow-hidden">
              {credential.isCurrent && <Badge variant="success" className="whitespace-nowrap">当前</Badge>}
              <Badge variant="outline" className="max-w-[120px] truncate whitespace-nowrap" title={credential.endpoint}>
                {credential.endpoint}
              </Badge>
            </div>
          </div>
        </td>
        <td className="max-w-[180px] px-3 py-3">
          <div className="flex items-center gap-2 overflow-hidden">
            <Badge variant={status.variant} className="whitespace-nowrap" title={status.title}>{status.text}</Badge>
            {disabledReason && <Badge variant="outline" className="max-w-[100px] truncate whitespace-nowrap" title={disabledReason.title}>{disabledReason.text}</Badge>}
          </div>
        </td>
        <td className="max-w-[140px] px-3 py-3">
          <Badge variant={credential.dispatchPath === 'soft_fallback' ? 'warning' : 'outline'} className="max-w-full truncate whitespace-nowrap" title={dispatchPathMeta.title}>
            {dispatchPathMeta.text}
          </Badge>
        </td>
        <td className="w-[170px] px-3 py-3">
          <div className="space-y-2">
            <div className="flex items-center justify-between gap-2 text-xs">
              <span className="truncate text-muted-foreground">并发</span>
              <span className="whitespace-nowrap font-medium">{credential.currentConcurrent}/{credential.maxConcurrent}</span>
            </div>
            <Progress value={progressValue} />
          </div>
        </td>
        <td className="max-w-[140px] px-3 py-3">
          <CellText title={credential.lastUsedAt ?? '从未使用'}>{formatLastUsed(credential.lastUsedAt)}</CellText>
        </td>
        <td className="max-w-[140px] px-3 py-3">
          <div className="space-y-1">
            {rateLimit ? (
              <Badge variant={rateLimit.variant} className="max-w-full truncate whitespace-nowrap" title={rateLimit.title}>
                {rateLimit.text}
              </Badge>
            ) : (
              <CellText>无</CellText>
            )}
            {credential.cooldownRemainingMs ? (
              <CellText className="text-xs text-muted-foreground" title="冷却结束后会自动恢复">
                {formatCooldown(credential.cooldownRemainingMs)}
              </CellText>
            ) : null}
          </div>
        </td>
        <td className="max-w-[160px] px-3 py-3">
          <CellText title={credential.stickyDetached ? '风控触发后，会话已自动切走。' : '当前仍保留会话绑定。'}>
            {credential.stickyDetached ? '已解除粘性' : `${credential.stickySessionCount} 个活跃会话`}
          </CellText>
        </td>
        <td className="w-[90px] px-3 py-3">
          <CellText>{String(credential.priority)}</CellText>
        </td>
        <td className="max-w-[120px] px-3 py-3">
          <CellText title={authMethod?.title ?? balanceLabel(credential)}>
            {authMethod?.text ?? balanceLabel(credential)}
          </CellText>
        </td>
        <td className="max-w-[120px] px-3 py-3">
          <div className="flex items-center gap-2">
            <span className="shrink-0 text-xs text-muted-foreground">参与调度</span>
            <Switch
              checked={!credential.disabled}
              onCheckedChange={handleToggleDisabled}
              disabled={setDisabled.isPending}
            />
          </div>
        </td>
        <td className="w-[260px] px-3 py-3">
          <div className="flex items-center justify-end gap-2 whitespace-nowrap">
            <Button size="sm" variant="outline" onClick={() => setShowTestDialog(true)} title="测试这个账号此刻是否真的还能调用">
              <PlugZap className="h-4 w-4" />
            </Button>
            <Button size="sm" variant="outline" onClick={() => onViewBalance(credential.id)} title="查看余额">
              <Wallet className="h-4 w-4" />
            </Button>
            <Button size="sm" variant="outline" onClick={() => setShowSettingsDialog(true)} title="修改优先级和并发上限">
              <Settings2 className="h-4 w-4" />
            </Button>
            {canRecover && (
              <Button size="sm" variant="outline" onClick={handleRecover} disabled={recoverCredential.isPending} title="清理本地阻塞">
                <ShieldAlert className="h-4 w-4" />
              </Button>
            )}
            {canRefresh && (
              <Button size="sm" variant="outline" onClick={handleRefreshToken} disabled={forceRefresh.isPending} title="强制刷新 Token">
                <RefreshCw className={cn('h-4 w-4', forceRefresh.isPending && 'animate-spin')} />
              </Button>
            )}
            <Button size="sm" variant="destructive" onClick={() => setShowDeleteDialog(true)} title="删除这个账号">
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        </td>
      </tr>

      <Dialog open={showSettingsDialog} onOpenChange={setShowSettingsDialog}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>账号设置</DialogTitle>
            <DialogDescription className="truncate whitespace-nowrap" title={`${credential.email || `凭据 #${credential.id}`} · ${credential.endpoint}`}>
              {credential.email || `凭据 #${credential.id}`} · {credential.endpoint}
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium">优先级</label>
              <Input type="number" min="0" value={priorityValue} onChange={(e) => setPriorityValue(e.target.value)} />
              <p className="truncate text-xs text-muted-foreground">数字越小越优先。</p>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">并发上限</label>
              <Input type="number" min="1" value={maxConcurrentValue} onChange={(e) => setMaxConcurrentValue(e.target.value)} />
              <p className="truncate text-xs text-muted-foreground">账号同时承接请求的最大数量。</p>
            </div>
          </div>
          <div className="grid gap-3 md:grid-cols-2">
            {infoItems.map((item) => (
              <div key={item.label} className="rounded-md border bg-muted/20 px-3 py-3">
                <div className="truncate text-xs text-muted-foreground" title={item.title}>{item.label}</div>
                <div className="mt-1 truncate text-sm font-medium" title={item.value}>{item.value}</div>
              </div>
            ))}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowSettingsDialog(false)}>取消</Button>
            <Button onClick={handleSaveSettings} disabled={setPriority.isPending || setMaxConcurrent.isPending}>保存设置</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={showTestDialog} onOpenChange={setShowTestDialog}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>测试接入</DialogTitle>
            <DialogDescription className="truncate whitespace-nowrap" title={`${credential.email || `凭据 #${credential.id}`} · ${credential.endpoint}`}>
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
                    <div className="truncate font-medium">{credential.email || `凭据 #${credential.id}`}</div>
                    <div className="mt-1 flex gap-2 overflow-hidden text-xs text-muted-foreground">
                      <CellText>{authMethod?.text ?? '账号接入'}</CellText>
                      <CellText>{credential.endpoint}</CellText>
                      <CellText>{`并发 ${credential.currentConcurrent}/${credential.maxConcurrent}`}</CellText>
                      {rateLimit && <CellText>{rateLimit.text}</CellText>}
                    </div>
                  </div>
                </div>
                <Badge variant={status.variant} title={status.title} className="whitespace-nowrap">
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
              <div className="mb-3 flex items-center justify-between gap-3">
                <div className="truncate text-[11px] text-muted-foreground">实时输出</div>
                <div className="flex max-w-[70%] gap-2 overflow-hidden text-[11px] text-muted-foreground">
                  <CellText>{`测试模型：${modelOptionsFor(credential).find((item) => item.value === testModel)?.label ?? testModel}`}</CellText>
                  <CellText>{testPrompt.trim() ? `提示词：${testPrompt.trim()}` : '提示词：默认检查语句'}</CellText>
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
            <Button variant="outline" onClick={() => setShowTestDialog(false)} disabled={testing}>关闭</Button>
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
              删除后无法恢复。如果这是当前正在使用的账号，系统会自动切换到其他可用账号。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowDeleteDialog(false)} disabled={deleteCredential.isPending}>取消</Button>
            <Button variant="destructive" onClick={handleDelete} disabled={deleteCredential.isPending}>确认删除</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
