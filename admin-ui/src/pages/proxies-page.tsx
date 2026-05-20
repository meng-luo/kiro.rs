import { useEffect, useState } from 'react'
import { Network, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { MetricCard } from '@/components/metric-card'
import { useCreateProxy, useDeleteProxy, useProxies, useTestProxy, useUpdateProxy } from '@/hooks/use-credentials'
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

function status(proxy: ProxyListItem) {
  if (proxy.disabled) return <Badge variant="outline">已停用</Badge>
  if (proxy.lastTestStatus === 'ok') return <Badge variant="success">可用</Badge>
  if (proxy.lastTestStatus === 'failed') return <Badge variant="destructive">不可用</Badge>
  return <Badge variant="secondary">未测试</Badge>
}

export function ProxiesPage() {
  const proxies = useProxies()
  const createProxy = useCreateProxy()
  const updateProxy = useUpdateProxy()
  const deleteProxy = useDeleteProxy()
  const testProxy = useTestProxy()
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<ProxyListItem | null>(null)
  const [form, setForm] = useState<ProxyUpsertRequest>(emptyForm)

  useEffect(() => {
    if (!dialogOpen) return
    if (editing) {
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
      setForm(emptyForm)
    }
  }, [dialogOpen, editing])

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

  const list = proxies.data?.proxies ?? []

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">代理</h1>
          <p className="mt-1 text-sm text-muted-foreground">维护代理池，并为不同账号选择不同连接。</p>
        </div>
        <Button onClick={() => { setEditing(null); setDialogOpen(true) }}>
          <Plus className="h-4 w-4" />
          添加代理
        </Button>
      </div>

      <div className="grid gap-3 md:grid-cols-3">
        <MetricCard label="代理总数" value={proxies.data?.total ?? 0} icon={Network} />
        <MetricCard label="已启用" value={proxies.data?.enabledCount ?? 0} />
        <MetricCard label="已绑定账号" value={list.reduce((sum, item) => sum + item.accountCount, 0)} />
      </div>

      <Card className="rounded-md">
        <CardHeader>
          <CardTitle className="text-base">代理列表</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {list.length === 0 ? (
            <div className="rounded-md border py-12 text-center text-sm text-muted-foreground">还没有代理</div>
          ) : (
            list.map((proxy) => (
              <div key={proxy.id} className="rounded-md border p-4">
                <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <div className="truncate font-medium">{proxy.name}</div>
                      {status(proxy)}
                    </div>
                    <div className="mt-1 truncate text-sm text-muted-foreground">
                      {proxy.protocol}://{proxy.host}:{proxy.port} · {proxy.accountCount} 个账号
                    </div>
                    <div className="mt-1 truncate text-xs text-muted-foreground">
                      {proxy.lastTestedAt ? `上次测试 ${formatTime(proxy.lastTestedAt)}` : '还没有测试记录'}
                      {proxy.lastLatencyMs ? ` · ${proxy.lastLatencyMs}ms` : ''}
                    </div>
                    {proxy.lastError ? <div className="mt-2 truncate text-xs text-destructive" title={proxy.lastError}>{proxy.lastError}</div> : null}
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <Button size="sm" variant="outline" onClick={() => testProxy.mutate(proxy.id, {
                      onSuccess: (item) => toast.success(item.lastTestStatus === 'ok' ? '代理可用' : '测试完成'),
                      onError: (error) => toast.error(extractErrorMessage(error)),
                    })}>
                      <RefreshCw className="h-4 w-4" />
                      测试
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
            <Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} placeholder="名称" />
            <div className="grid gap-3 sm:grid-cols-[120px_minmax(0,1fr)_120px]">
              <select className="h-10 rounded-md border border-input bg-background px-3 text-sm" value={form.protocol} onChange={(event) => setForm({ ...form, protocol: event.target.value })}>
                <option value="http">http</option>
                <option value="https">https</option>
                <option value="socks5">socks5</option>
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
