import { useEffect, useState } from 'react'
import { Activity, Download, History, Power, RefreshCw, Save } from 'lucide-react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import {
  useAdminSettings,
  useCheckSystemVersion,
  useRestartSystem,
  useRollbackSystemVersion,
  useSetAdminSettings,
  useSchedulerConfig,
  useSetSchedulerConfig,
  useSystemJob,
  useSystemVersion,
  useUpdateSystemVersion,
} from '@/hooks/use-credentials'
import { storage, type AdminTheme } from '@/lib/storage'
import { extractErrorMessage } from '@/lib/utils'
import { formatTime } from '@/lib/format'
import type { SchedulerConfig } from '@/types/api'

function applyTheme(theme: AdminTheme) {
  const prefersDark = window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false
  document.documentElement.classList.toggle('dark', theme === 'dark' || (theme === 'system' && prefersDark))
}

function deploymentModeLabel(mode?: string) {
  if (mode === 'docker') return '容器部署'
  if (mode === 'binary') return '二进制部署'
  if (mode === 'file') return '文件部署'
  return mode || '未知'
}

function jobLabel(status?: string) {
  switch (status) {
    case 'running':
      return <Badge variant="warning">执行中</Badge>
    case 'succeeded':
      return <Badge variant="success">已完成</Badge>
    case 'failed':
      return <Badge variant="destructive">失败</Badge>
    case 'rolled_back':
      return <Badge variant="outline">已回滚</Badge>
    default:
      return <Badge variant="secondary">暂无任务</Badge>
  }
}

export function SettingsPage() {
  const settings = useAdminSettings()
  const setSettings = useSetAdminSettings()
  const scheduler = useSchedulerConfig()
  const setScheduler = useSetSchedulerConfig()
  const version = useSystemVersion()
  const checkVersion = useCheckSystemVersion()
  const updateSystem = useUpdateSystemVersion()
  const rollbackSystem = useRollbackSystemVersion()
  const restartSystem = useRestartSystem()
  const [theme, setTheme] = useState<AdminTheme>('system')
  const [redisUrl, setRedisUrl] = useState('')
  const [schedulerForm, setSchedulerForm] = useState<SchedulerConfig | null>(null)
  const [activeJobId, setActiveJobId] = useState<string | null>(null)
  const activeJob = useSystemJob(activeJobId, Boolean(activeJobId))
  const currentJob = activeJob.data || version.data?.latestJob || null

  useEffect(() => {
    if (!settings.data) return
    setTheme(settings.data.theme)
    setRedisUrl(settings.data.promptCache.redisUrl ?? '')
  }, [settings.data])

  useEffect(() => {
    if (!scheduler.data?.config) return
    setSchedulerForm(scheduler.data.config)
  }, [scheduler.data?.config])

  useEffect(() => {
    if (version.data?.latestJob?.jobId) setActiveJobId((prev) => prev ?? version.data?.latestJob?.jobId ?? null)
  }, [version.data?.latestJob?.jobId])

  const saveSettings = () => {
    setSettings.mutate(
      { theme, redisUrl: redisUrl.trim() || null },
      {
        onSuccess: (response) => {
          storage.setTheme(response.theme)
          applyTheme(response.theme)
          toast.success('设置已保存')
        },
        onError: (error) => toast.error(extractErrorMessage(error)),
      },
    )
  }

  const saveScheduler = () => {
    if (!schedulerForm) return
    setScheduler.mutate(schedulerForm, {
      onSuccess: () => toast.success('调度设置已保存'),
      onError: (error) => toast.error(extractErrorMessage(error)),
    })
  }

  const updateSchedulerNumber = (key: keyof SchedulerConfig, value: string) => {
    if (!schedulerForm) return
    const next = Number(value)
    setSchedulerForm({
      ...schedulerForm,
      [key]: Number.isFinite(next) ? next : 0,
    })
  }

  const startJob = (type: 'update' | 'rollback' | 'restart') => {
    const mutation =
      type === 'update'
        ? updateSystem.mutateAsync(version.data?.updateAvailable ? { version: version.data.latestVersion } : {})
        : type === 'rollback'
          ? rollbackSystem.mutateAsync({})
          : restartSystem.mutateAsync()
    mutation
      .then((job) => {
        setActiveJobId(job.jobId)
        toast.success('任务已开始')
      })
      .catch((error) => toast.error(extractErrorMessage(error)))
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">设置</h1>
        <p className="mt-1 text-sm text-muted-foreground">保存界面偏好、缓存连接和维护操作。</p>
      </div>

      <div className="grid gap-6 xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
        <Card className="rounded-md">
          <CardHeader>
            <CardTitle className="text-base">界面与缓存</CardTitle>
          </CardHeader>
          <CardContent className="space-y-5">
            <div className="space-y-2">
              <label className="text-sm font-medium">主题</label>
              <select
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
                value={theme}
                onChange={(event) => setTheme(event.target.value as AdminTheme)}
              >
                <option value="system">跟随系统</option>
                <option value="light">浅色</option>
                <option value="dark">深色</option>
              </select>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Redis 地址</label>
              <Input value={redisUrl} onChange={(event) => setRedisUrl(event.target.value)} placeholder="redis://127.0.0.1:6379/0" />
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <Badge variant={settings.data?.promptCache.connected ? 'success' : 'outline'}>
                  {settings.data?.promptCache.connected ? '已连接' : '未连接'}
                </Badge>
                <span>留空保存后会关闭缓存显示。</span>
              </div>
              {settings.data?.promptCache.lastError ? (
                <div className="truncate text-xs text-destructive" title={settings.data.promptCache.lastError}>
                  {settings.data.promptCache.lastError}
                </div>
              ) : null}
            </div>
            <Button onClick={saveSettings} disabled={setSettings.isPending}>
              {setSettings.isPending ? '保存中...' : '保存设置'}
            </Button>
          </CardContent>
        </Card>

        <Card className="rounded-md">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <Activity className="h-4 w-4" />
              请求调度
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-5">
            <div className="grid gap-3 md:grid-cols-3">
              {(scheduler.data?.models ?? []).slice(0, 6).map((item) => (
                <div key={item.model} className="rounded-md border p-3">
                  <div className="truncate text-xs text-muted-foreground" title={item.model}>{item.model}</div>
                  <div className="mt-1 font-medium">{item.inflight}/{item.window}</div>
                  <div className="mt-1 text-xs text-muted-foreground">
                    {item.backoffRemainingMs > 0 ? `等待 ${(item.backoffRemainingMs / 1000).toFixed(1)} 秒` : '可继续接收'}
                  </div>
                </div>
              ))}
              {scheduler.data?.models.length === 0 ? (
                <div className="rounded-md border p-3 text-sm text-muted-foreground">还没有请求记录</div>
              ) : null}
            </div>

            {schedulerForm ? (
              <div className="space-y-4">
                <label className="flex items-center justify-between gap-4 rounded-md border p-3">
                  <span>
                    <span className="block text-sm font-medium">自动放慢重试</span>
                    <span className="block text-xs text-muted-foreground">服务繁忙时先等待，再继续尝试返回结果。</span>
                  </span>
                  <input
                    type="checkbox"
                    checked={schedulerForm.enabled}
                    onChange={(event) => setSchedulerForm({ ...schedulerForm, enabled: event.target.checked })}
                    className="h-4 w-4"
                  />
                </label>

                <div className="grid gap-3 md:grid-cols-2">
                  <div className="space-y-2">
                    <label className="text-sm font-medium">最长等待时间（毫秒）</label>
                    <Input type="number" min="1000" value={schedulerForm.requestBudgetMs} onChange={(event) => updateSchedulerNumber('requestBudgetMs', event.target.value)} />
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">排队等待时间（毫秒）</label>
                    <Input type="number" min="1000" value={schedulerForm.queueTimeoutMs} onChange={(event) => updateSchedulerNumber('queueTimeoutMs', event.target.value)} />
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">最多尝试次数</label>
                    <Input type="number" min="1" max="9" value={schedulerForm.maxAttemptsPerRequest} onChange={(event) => updateSchedulerNumber('maxAttemptsPerRequest', event.target.value)} />
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">繁忙后首次等待（毫秒）</label>
                    <Input type="number" min="100" value={schedulerForm.normal429BackoffInitialMs} onChange={(event) => updateSchedulerNumber('normal429BackoffInitialMs', event.target.value)} />
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">繁忙后最长等待（毫秒）</label>
                    <Input type="number" min="1000" value={schedulerForm.normal429BackoffMaxMs} onChange={(event) => updateSchedulerNumber('normal429BackoffMaxMs', event.target.value)} />
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">账号短暂休息（毫秒）</label>
                    <Input type="number" min="1000" value={schedulerForm.normal429AccountCooldownMs} onChange={(event) => updateSchedulerNumber('normal429AccountCooldownMs', event.target.value)} />
                  </div>
                </div>

                <label className="flex items-center justify-between gap-4 rounded-md border p-3">
                  <span>
                    <span className="block text-sm font-medium">流式请求加速试探</span>
                    <span className="block text-xs text-muted-foreground">等待后仍未返回时，额外尝试一次更快拿到结果。</span>
                  </span>
                  <input
                    type="checkbox"
                    checked={schedulerForm.hedgeEnabled}
                    onChange={(event) => setSchedulerForm({ ...schedulerForm, hedgeEnabled: event.target.checked })}
                    className="h-4 w-4"
                  />
                </label>

                <div className="grid gap-3 md:grid-cols-2">
                  <div className="space-y-2">
                    <label className="text-sm font-medium">加速试探等待（毫秒）</label>
                    <Input type="number" min="500" value={schedulerForm.hedgeDelayMs} onChange={(event) => updateSchedulerNumber('hedgeDelayMs', event.target.value)} />
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">额外尝试次数</label>
                    <Input type="number" min="0" max="1" value={schedulerForm.hedgeMaxExtraPerRequest} onChange={(event) => updateSchedulerNumber('hedgeMaxExtraPerRequest', event.target.value)} />
                  </div>
                </div>

                <Button onClick={saveScheduler} disabled={setScheduler.isPending}>
                  <Save className="h-4 w-4" />
                  {setScheduler.isPending ? '保存中...' : '保存调度设置'}
                </Button>
              </div>
            ) : (
              <div className="text-sm text-muted-foreground">正在读取调度设置...</div>
            )}
          </CardContent>
        </Card>

        <Card className="rounded-md">
          <CardHeader>
            <CardTitle className="text-base">版本与维护</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-3 md:grid-cols-2">
              <div className="rounded-md border p-3">
                <div className="text-xs text-muted-foreground">当前版本</div>
                <div className="mt-1 font-medium">{version.data?.currentVersion ?? '-'}</div>
              </div>
              <div className="rounded-md border p-3">
                <div className="text-xs text-muted-foreground">最新版本</div>
                <div className="mt-1 flex items-center gap-2">
                  <span className="font-medium">{version.data?.latestVersion ?? '-'}</span>
                  {version.data?.updateAvailable ? <Badge variant="warning">可更新</Badge> : null}
                </div>
              </div>
              <div className="rounded-md border p-3">
                <div className="text-xs text-muted-foreground">部署方式</div>
                <div className="mt-1 font-medium">{deploymentModeLabel(version.data?.deploymentMode)}</div>
              </div>
              <div className="rounded-md border p-3">
                <div className="text-xs text-muted-foreground">检查时间</div>
                <div className="mt-1 font-medium">{formatTime(version.data?.checkedAt)}</div>
              </div>
            </div>

            <div className="rounded-md border p-4">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <div className="font-medium">最近任务</div>
                  <div className="mt-1 text-sm text-muted-foreground">{currentJob?.message || '还没有维护任务'}</div>
                </div>
                {jobLabel(currentJob?.status)}
              </div>
            </div>

            <div className="flex flex-wrap gap-2">
              <Button variant="outline" onClick={() => checkVersion.mutate(undefined, {
                onSuccess: () => toast.success('版本信息已刷新'),
                onError: (error) => toast.error(extractErrorMessage(error)),
              })}>
                <RefreshCw className="h-4 w-4" />
                刷新信息
              </Button>
              <Button variant="outline" onClick={() => startJob('update')} disabled={!version.data?.canUpdate}>
                <Download className="h-4 w-4" />
                更新
              </Button>
              <Button variant="outline" onClick={() => startJob('rollback')} disabled={!version.data?.canRollback}>
                <History className="h-4 w-4" />
                回滚
              </Button>
              <Button variant="outline" onClick={() => startJob('restart')} disabled={!version.data?.canRestart}>
                <Power className="h-4 w-4" />
                重启
              </Button>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
