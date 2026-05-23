import { ChangeEvent, useEffect, useRef, useState } from 'react'
import { Check, Copy, Download, FileUp, Network, PlugZap, Plus, RefreshCw, ShieldCheck, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { MetricCard } from '@/components/metric-card'
import { useAdminSettings, useBatchDeleteProxies, useBatchQualityCheckProxies, useBatchTestProxies, useCreateProxy, useDeleteProxy, useProxies, useSetDefaultConnection, useTestProxy, useUpdateProxy } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import { formatTime } from '@/lib/format'
import type { ProxyListItem, ProxyUpsertRequest } from '@/types/api'

const emptyForm: ProxyUpsertRequest = {
  name: '',
  protocol: 'http',
  host: '',
  port: 7890,
  username: '',
  password: '',
  disabled: false,
}

const proxyProtocols = ['http', 'https', 'socks5', 'socks5h']

function normalizeProxyProtocol(value: string) {
  const protocol = value.replace(/:$/, '').trim().toLowerCase()
  return proxyProtocols.includes(protocol) ? protocol : null
}

function parseProxyAddress(value: string): Partial<ProxyUpsertRequest> | null {
  const raw = value.trim()
  if (!raw) return null

  const parseUrl = (source: string): Partial<ProxyUpsertRequest> | null => {
    try {
      const url = new URL(source)
      const protocol = normalizeProxyProtocol(url.protocol)
      const port = Number(url.port)
      if (!protocol || !url.hostname || !Number.isInteger(port) || port <= 0 || port > 65535) return null
      return {
        protocol,
        host: url.hostname,
        port,
        username: url.username ? decodeURIComponent(url.username) : '',
        password: url.password ? decodeURIComponent(url.password) : '',
      }
    } catch {
      return null
    }
  }

  if (raw.includes('://')) return parseUrl(raw)
  if (raw.includes('@')) return parseUrl(`http://${raw}`)

  const parts = raw.split(':').map((part) => part.trim())
  if (parts.length !== 2 && parts.length !== 4) return null

  const [host, portText, username = '', password = ''] = parts
  const port = Number(portText)
  if (!host || !Number.isInteger(port) || port <= 0 || port > 65535) return null
  return { protocol: 'http', host, port, username, password }
}

function status(proxy: ProxyListItem) {
  if (proxy.disabled) return <Badge variant="outline">已停用</Badge>
  if (proxy.lastTestStatus === 'ok') return <Badge variant="success">可用</Badge>
  if (proxy.lastTestStatus === 'failed') return <Badge variant="destructive">不可用</Badge>
  return <Badge variant="secondary">未测试</Badge>
}

function cardTone(proxy: ProxyListItem) {
  if (proxy.disabled) return 'border-gray-300 bg-gray-50/70 dark:bg-muted/20'
  if (proxy.lastTestStatus === 'failed') return 'border-red-300 bg-red-50/70 dark:bg-red-950/10'
  if (proxy.qualityScore && proxy.qualityScore < 75) return 'border-yellow-300 bg-yellow-50/70 dark:bg-yellow-950/10'
  return 'border-green-300 bg-green-50/60 dark:bg-green-950/10'
}

export function ProxiesPage() {
  const proxies = useProxies()
  const settings = useAdminSettings()
  const createProxy = useCreateProxy()
  const updateProxy = useUpdateProxy()
  const setDefaultConnection = useSetDefaultConnection()
  const deleteProxy = useDeleteProxy()
  const testProxy = useTestProxy()
  const batchTest = useBatchTestProxies()
  const batchDelete = useBatchDeleteProxies()
  const batchQuality = useBatchQualityCheckProxies()
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<ProxyListItem | null>(null)
  const [form, setForm] = useState<ProxyUpsertRequest>(emptyForm)
  const [quickProxy, setQuickProxy] = useState('')
  const [query, setQuery] = useState('')
  const [protocol, setProtocol] = useState('all')
  const [state, setState] = useState('all')
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set())
  const importInputRef = useRef<HTMLInputElement>(null)

  const defaultConnection = settings.data?.defaultConnection
  const defaultConnectionIsCustom = defaultConnection?.mode === 'proxy' && !defaultConnection.proxyId
  const defaultConnectionValue =
    defaultConnection?.mode === 'proxy' && defaultConnection.proxyId
      ? `proxy:${defaultConnection.proxyId}`
      : defaultConnectionIsCustom
        ? 'custom:'
      : 'direct:'

  useEffect(() => {
    if (!dialogOpen) return
    if (editing) {
      setQuickProxy('')
      setForm({
        name: editing.name,
        protocol: editing.protocol,
        host: editing.host,
        port: editing.port,
        username: editing.username ?? '',
        password: '',
        disabled: editing.disabled,
      })
    } else {
      setQuickProxy('')
      setForm(emptyForm)
    }
  }, [dialogOpen, editing])

  const applyQuickProxy = (value: string) => {
    setQuickProxy(value)
    const parsed = parseProxyAddress(value)
    if (!parsed) return
    setForm((prev) => ({
      ...prev,
      ...parsed,
      name: prev.name.trim() || parsed.host || prev.name,
    }))
  }

  const save = () => {
    const payload = {
      ...form,
      username: form.username?.trim() || null,
      password: form.password?.trim() || null,
    }
    const mutation = editing
      ? updateProxy.mutateAsync({ id: editing.id, payload })
      : createProxy.mutateAsync(payload)
    mutation
      .then(() => {
        toast.success(editing ? '代理已更新' : '代理已添加')
        setDialogOpen(false)
      })
      .catch((error) => toast.error(extractErrorMessage(error)))
  }

  const list = (proxies.data?.proxies ?? []).filter((proxy) => {
    const keyword = query.trim().toLowerCase()
    if (keyword && !`${proxy.name} ${proxy.host} ${proxy.port}`.toLowerCase().includes(keyword)) return false
    if (protocol !== 'all' && proxy.protocol !== protocol) return false
    if (state === 'enabled' && proxy.disabled) return false
    if (state === 'disabled' && !proxy.disabled) return false
    if (state === 'failed' && proxy.lastTestStatus !== 'failed') return false
    if (state === 'unknown' && proxy.lastTestStatus && proxy.lastTestStatus !== 'unknown') return false
    return true
  })
  const visibleIds = list.map((item) => item.id)
  const allVisibleSelected = visibleIds.length > 0 && visibleIds.every((id) => selectedIds.has(id))

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const toggleVisible = () => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (allVisibleSelected) visibleIds.forEach((id) => next.delete(id))
      else visibleIds.forEach((id) => next.add(id))
      return next
    })
  }

  const selectedArray = Array.from(selectedIds)

  const copyProxy = async (proxy: ProxyListItem) => {
    const auth = proxy.username ? `${proxy.username}@` : ''
    await navigator.clipboard.writeText(`${proxy.protocol}://${auth}${proxy.host}:${proxy.port}`)
    toast.success('代理地址已复制')
  }

  const testSelected = async () => {
    const result = await batchTest.mutateAsync({ ids: selectedArray })
    toast.success(`测试完成：${result.successCount}/${selectedArray.length} 个可用`)
  }

  const deleteSelected = async () => {
    if (!confirm(`确定删除 ${selectedArray.length} 个代理吗？已绑定账号的代理不会被删除。`)) return
    const result = await batchDelete.mutateAsync({ ids: selectedArray })
    setSelectedIds(new Set())
    toast.success(`已删除 ${result.successCount}/${selectedArray.length} 个代理`)
  }

  const qualitySelected = async () => {
    const result = await batchQuality.mutateAsync({ ids: selectedArray })
    toast.success(`检测完成：${result.successCount}/${selectedArray.length} 个通过`)
  }

  const exportProxies = () => {
    const payload = list.map((proxy) => ({
      name: proxy.name,
      protocol: proxy.protocol,
      host: proxy.host,
      port: proxy.port,
      username: proxy.username ?? null,
      disabled: proxy.disabled,
    }))
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = `kiro-proxies-${Date.now()}.json`
    link.click()
    URL.revokeObjectURL(url)
  }

  const changeDefaultConnection = (value: string) => {
    const [mode, proxyId] = value.split(':')
    setDefaultConnection.mutate(
      {
        mode: mode === 'proxy' ? 'proxy' : 'direct',
        proxyId: mode === 'proxy' ? Number(proxyId) : null,
      },
      {
        onSuccess: () => toast.success('默认连接已更新'),
        onError: (error) => toast.error(extractErrorMessage(error)),
      },
    )
  }

  const parseProxyImport = (text: string): ProxyUpsertRequest[] => {
    const raw = JSON.parse(text)
    const items = Array.isArray(raw) ? raw : raw?.proxies
    if (!Array.isArray(items)) throw new Error('请选择代理列表 JSON 文件')
    return items.map((item, index) => {
      const name = String(item.name ?? `代理 ${index + 1}`).trim()
      const protocol = String(item.protocol ?? 'http').trim().toLowerCase()
      const host = String(item.host ?? '').trim()
      const port = Number(item.port)
      if (!name || !host || !Number.isInteger(port) || port <= 0 || port > 65535) {
        throw new Error(`第 ${index + 1} 条代理格式不正确`)
      }
      return {
        name,
        protocol,
        host,
        port,
        username: item.username ? String(item.username).trim() : null,
        password: item.password ? String(item.password) : null,
        disabled: Boolean(item.disabled),
      }
    })
  }

  const importProxies = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    event.target.value = ''
    if (!file) return
    try {
      const payloads = parseProxyImport(await file.text())
      let success = 0
      for (const payload of payloads) {
        await createProxy.mutateAsync(payload)
        success += 1
      }
      toast.success(`已导入 ${success} 个代理`)
    } catch (error) {
      toast.error(extractErrorMessage(error))
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">代理</h1>
          <p className="mt-1 text-sm text-muted-foreground">维护代理池，并为不同账号选择不同连接。</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" onClick={() => proxies.refetch()}><RefreshCw className="h-4 w-4" />刷新</Button>
          <Button variant="outline" onClick={testSelected} disabled={selectedIds.size === 0 || batchTest.isPending}><RefreshCw className="h-4 w-4" />批量测试</Button>
          <Button variant="outline" onClick={qualitySelected} disabled={selectedIds.size === 0 || batchQuality.isPending}><ShieldCheck className="h-4 w-4" />批量质检</Button>
          <Button variant="destructive" onClick={deleteSelected} disabled={selectedIds.size === 0 || batchDelete.isPending}><Trash2 className="h-4 w-4" />批量删除</Button>
          <input ref={importInputRef} type="file" accept="application/json,.json" className="hidden" onChange={importProxies} />
          <Button variant="outline" onClick={() => importInputRef.current?.click()} disabled={createProxy.isPending}><FileUp className="h-4 w-4" />导入</Button>
          <Button variant="outline" onClick={exportProxies} disabled={list.length === 0}><Download className="h-4 w-4" />导出</Button>
          <Button onClick={() => { setEditing(null); setDialogOpen(true) }}><Plus className="h-4 w-4" />创建代理</Button>
        </div>
      </div>

      <div className="grid gap-3 md:grid-cols-3">
        <MetricCard label="代理总数" value={proxies.data?.total ?? 0} icon={Network} />
        <MetricCard label="已启用" value={proxies.data?.enabledCount ?? 0} />
        <MetricCard label="已绑定账号" value={list.reduce((sum, item) => sum + item.accountCount, 0)} />
      </div>

      <Card className="rounded-md">
        <CardContent className="flex flex-col gap-3 p-4 lg:flex-row lg:items-center lg:justify-between">
          <div className="flex min-w-0 items-center gap-3">
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-emerald-50 text-emerald-700 dark:bg-emerald-950/30 dark:text-emerald-300">
              <PlugZap className="h-4 w-4" />
            </div>
            <div className="min-w-0">
              <div className="text-sm font-medium">默认连接</div>
              <div className="truncate text-xs text-muted-foreground">
                {defaultConnection?.mode === 'proxy'
                  ? (defaultConnection.proxyName || defaultConnection.proxyUrl || '自定义代理')
                  : '直连'}
              </div>
            </div>
          </div>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
            <select
              className="h-10 min-w-[220px] rounded-md border border-input bg-background px-3 text-sm"
              value={defaultConnectionValue}
              onChange={(event) => changeDefaultConnection(event.target.value)}
              disabled={setDefaultConnection.isPending}
            >
              {defaultConnectionIsCustom ? <option value="custom:" disabled>当前自定义代理</option> : null}
              <option value="direct:">直连</option>
              {list.filter((item) => !item.disabled).map((item) => (
                <option key={item.id} value={`proxy:${item.id}`}>
                  {item.name} · {item.host}:{item.port}
                </option>
              ))}
            </select>
            {defaultConnectionIsCustom ? (
              <Badge variant="outline">自定义代理</Badge>
            ) : null}
          </div>
        </CardContent>
      </Card>

      <Card className="rounded-md">
        <CardContent className="grid gap-3 p-4 md:grid-cols-[minmax(0,1fr)_160px_160px_auto]">
          <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索名称或地址" />
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={protocol} onChange={(event) => setProtocol(event.target.value)}>
            <option value="all">全部协议</option>
            <option value="http">http</option>
            <option value="https">https</option>
            <option value="socks5">socks5</option>
            <option value="socks5h">socks5h</option>
          </select>
          <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={state} onChange={(event) => setState(event.target.value)}>
            <option value="all">全部状态</option>
            <option value="enabled">已启用</option>
            <option value="disabled">已停用</option>
            <option value="failed">不可用</option>
            <option value="unknown">未测试</option>
          </select>
          <Button variant="outline" onClick={toggleVisible} disabled={visibleIds.length === 0}>
            <Check className="h-4 w-4" />
            {allVisibleSelected ? '取消选择' : '选择结果'}
          </Button>
        </CardContent>
      </Card>

      {selectedIds.size > 0 ? (
        <div className="rounded-md bg-blue-50 p-3 dark:bg-blue-950/20">
          <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <span className="font-medium text-blue-900 dark:text-blue-100">已选中 {selectedIds.size} 个代理</span>
              <span className="text-blue-200">•</span>
              <button className="text-xs font-medium text-blue-700 hover:text-blue-800" onClick={toggleVisible}>选择当前页</button>
              <span className="text-blue-200">•</span>
              <button className="text-xs font-medium text-blue-700 hover:text-blue-800" onClick={() => setSelectedIds(new Set())}>清空选择</button>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="destructive" onClick={deleteSelected} disabled={batchDelete.isPending}>
                <Trash2 className="h-4 w-4" />
                删除
              </Button>
              <Button size="sm" variant="outline" onClick={testSelected} disabled={batchTest.isPending}>
                <RefreshCw className="h-4 w-4" />
                测试连接
              </Button>
              <Button size="sm" variant="outline" onClick={qualitySelected} disabled={batchQuality.isPending}>
                <ShieldCheck className="h-4 w-4" />
                质量检测
              </Button>
            </div>
          </div>
        </div>
      ) : null}

      <Card className="rounded-md">
        <CardHeader>
          <CardTitle className="text-base">代理列表</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {list.length === 0 ? (
            <div className="rounded-md border py-12 text-center text-sm text-muted-foreground">还没有代理</div>
          ) : (
            list.map((proxy) => (
              <div key={proxy.id} className={`rounded-md border p-4 ${cardTone(proxy)}`}>
                <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
                  <div className="flex min-w-0 gap-3">
                    <Checkbox checked={selectedIds.has(proxy.id)} onCheckedChange={() => toggleSelect(proxy.id)} />
                    <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <div className="truncate font-medium">{proxy.name}</div>
                      {proxy.isDefault ? <Badge variant="success">默认</Badge> : null}
                      {status(proxy)}
                    </div>
                    <div className="mt-1 truncate text-sm text-muted-foreground">
                      <code className="text-xs">{proxy.host}:{proxy.port}</code> · {proxy.accountCount} 个账号
                    </div>
                    <div className="mt-1 truncate text-xs text-muted-foreground">
                      {proxy.lastTestedAt ? `上次测试 ${formatTime(proxy.lastTestedAt)}` : '还没有测试记录'}
                      {proxy.lastLatencyMs ? ` · ${proxy.lastLatencyMs}ms` : ''}
                    </div>
                    {proxy.lastError ? <div className="mt-2 truncate text-xs text-destructive" title={proxy.lastError}>{proxy.lastError}</div> : null}
                    </div>
                  </div>
                  <div className="grid min-w-[260px] gap-2 text-xs text-muted-foreground sm:grid-cols-2">
                    <div>
                      <div>出口 IP</div>
                      <div className="font-medium text-foreground">{proxy.exitIp || '未获取'}</div>
                    </div>
                    <div>
                      <div>位置</div>
                      <div className="font-medium text-foreground">{[proxy.country, proxy.city].filter(Boolean).join(' · ') || '未获取'}</div>
                    </div>
                    <div>
                      <div>质量</div>
                      <div className="font-medium text-foreground">{proxy.qualityScore ? `${proxy.qualityGrade || '-'} (${proxy.qualityScore})` : '未检测'}</div>
                    </div>
                    <div>
                      <div>认证</div>
                      <div className="font-medium text-foreground">{proxy.username ? `${proxy.username} / ******` : '无'}</div>
                    </div>
                    <div>
                      <div>最近测试</div>
                      <div className="font-medium text-foreground">{proxy.lastTestedAt ? formatTime(proxy.lastTestedAt) : '未测试'}</div>
                    </div>
                    <div>
                      <div>状态</div>
                      <div className="font-medium text-foreground">{proxy.qualityError || (proxy.lastTestStatus === 'ok' ? '正常' : proxy.lastTestStatus === 'failed' ? '连接失败' : '未测试')}</div>
                    </div>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <Button size="sm" variant="outline" onClick={() => copyProxy(proxy)}>
                      <Copy className="h-4 w-4" />
                      复制
                    </Button>
                    <Button size="sm" variant="outline" onClick={() => testProxy.mutate(proxy.id, {
                      onSuccess: (item) => toast.success(item.lastTestStatus === 'ok' ? '代理可用' : '测试完成'),
                      onError: (error) => toast.error(extractErrorMessage(error)),
                    })}>
                      <RefreshCw className="h-4 w-4" />
                      测试
                    </Button>
                    <Button size="sm" variant="outline" onClick={() => batchQuality.mutate({ ids: [proxy.id] }, {
                      onSuccess: () => toast.success('质量检测完成'),
                      onError: (error) => toast.error(extractErrorMessage(error)),
                    })}>
                      <ShieldCheck className="h-4 w-4" />
                      质检
                    </Button>
                    <Button size="sm" variant="outline" onClick={() => { setEditing(proxy); setDialogOpen(true) }}>编辑</Button>
                    <Button
                      size="sm"
                      variant="destructive"
                      onClick={() => {
                        if (!confirm(`确定删除代理 ${proxy.name} 吗？`)) return
                        deleteProxy.mutate(proxy.id, {
                          onSuccess: () => toast.success('代理已删除'),
                          onError: (error) => toast.error(extractErrorMessage(error)),
                        })
                      }}
                    >
                      <Trash2 className="h-4 w-4" />
                      删除
                    </Button>
                  </div>
                </div>
              </div>
            ))
          )}
        </CardContent>
      </Card>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing ? '编辑代理' : '添加代理'}</DialogTitle>
          </DialogHeader>
          <div className="grid gap-4">
            <div className="space-y-2">
              <Input
                value={quickProxy}
                onChange={(event) => applyQuickProxy(event.target.value)}
                placeholder="粘贴代理地址，例如 76.9.106.231:5782:用户名:密码"
              />
              <p className="text-xs text-muted-foreground">粘贴后会自动填入下方信息，也支持 http://user:pass@host:port</p>
            </div>
            <Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} placeholder="名称" />
            <div className="grid gap-3 sm:grid-cols-[120px_minmax(0,1fr)_120px]">
              <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={form.protocol} onChange={(event) => setForm({ ...form, protocol: event.target.value })}>
                <option value="http">http</option>
                <option value="https">https</option>
                <option value="socks5">socks5</option>
                <option value="socks5h">socks5h</option>
              </select>
              <Input value={form.host} onChange={(event) => setForm({ ...form, host: event.target.value })} placeholder="地址" />
              <Input type="number" value={form.port} onChange={(event) => setForm({ ...form, port: Number(event.target.value) })} placeholder="端口" />
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <Input value={form.username ?? ''} onChange={(event) => setForm({ ...form, username: event.target.value })} placeholder="用户名" />
              <Input type="password" value={form.password ?? ''} onChange={(event) => setForm({ ...form, password: event.target.value })} placeholder={editing?.hasPassword ? '留空保持原密码' : '密码'} />
            </div>
            <label className="flex items-center justify-between rounded-md border p-3 text-sm">
              <span>启用这个代理</span>
              <Switch checked={!form.disabled} onCheckedChange={(checked) => setForm({ ...form, disabled: !checked })} />
            </label>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>取消</Button>
            <Button onClick={save} disabled={!form.name.trim() || !form.host.trim() || form.port <= 0}>保存</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
