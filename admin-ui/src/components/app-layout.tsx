import { NavLink, Outlet, useNavigate } from 'react-router-dom'
import { useQueryClient } from '@tanstack/react-query'
import {
  BarChart3,
  ListChecks,
  LogOut,
  Moon,
  Network,
  RefreshCw,
  Settings,
  Activity,
  Sun,
  Users,
} from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { storage } from '@/lib/storage'
import { cn, extractErrorMessage } from '@/lib/utils'
import {
  useAdminSettings,
  useCredentials,
  useLoadBalancingMode,
  useSetAdminSettings,
  useSetLoadBalancingMode,
} from '@/hooks/use-credentials'

interface AppLayoutProps {
  onLogout: () => void
}

const navItems = [
  { to: '/monitor', label: '监控', icon: Activity },
  { to: '/accounts', label: '账号', icon: Users },
  { to: '/stats', label: '统计', icon: BarChart3 },
  { to: '/records', label: '记录', icon: ListChecks },
  { to: '/proxies', label: '代理', icon: Network },
  { to: '/settings', label: '设置', icon: Settings },
]

function applyTheme(theme: 'light' | 'dark' | 'system') {
  const prefersDark = window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false
  document.documentElement.classList.toggle('dark', theme === 'dark' || (theme === 'system' && prefersDark))
}

export function AppLayout({ onLogout }: AppLayoutProps) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { data: credentials, refetch } = useCredentials()
  const { data: loadBalancingData, isLoading: isLoadingMode } = useLoadBalancingMode()
  const { data: settings } = useAdminSettings()
  const setLoadBalancingMode = useSetLoadBalancingMode()
  const setAdminSettings = useSetAdminSettings()

  const theme = settings?.theme ?? storage.getTheme() ?? 'system'

  const handleRefresh = () => {
    refetch()
    queryClient.invalidateQueries({ queryKey: ['diagnostics-summary'] })
    queryClient.invalidateQueries({ queryKey: ['diagnostics-requests'] })
    queryClient.invalidateQueries({ queryKey: ['proxies'] })
    toast.success('已刷新')
  }

  const handleTheme = () => {
    const next = theme === 'dark' ? 'light' : 'dark'
    storage.setTheme(next)
    applyTheme(next)
    setAdminSettings.mutate(
      { theme: next },
      {
        onSuccess: (response) => {
          storage.setTheme(response.theme)
          applyTheme(response.theme)
        },
        onError: (error) => toast.error(`保存失败: ${extractErrorMessage(error)}`),
      },
    )
  }

  const handleMode = () => {
    const next = loadBalancingData?.mode === 'priority' ? 'balanced' : 'priority'
    setLoadBalancingMode.mutate(next, {
      onSuccess: () => toast.success(next === 'balanced' ? '已切换为均衡负载' : '已切换为优先级模式'),
      onError: (error) => toast.error(`切换失败: ${extractErrorMessage(error)}`),
    })
  }

  const handleLogout = () => {
    storage.removeApiKey()
    queryClient.clear()
    onLogout()
  }

  return (
    <div className="min-h-screen bg-background">
      <header className="sticky top-0 z-50 border-b bg-background/95 backdrop-blur">
        <div className="mx-auto flex h-14 max-w-[1500px] items-center justify-between px-4 md:px-6">
          <div className="flex min-w-0 items-center gap-3">
            <button className="flex items-center gap-2" onClick={() => navigate('/monitor')}>
              <Activity className="h-5 w-5" />
              <span className="truncate font-semibold">Kiro Admin</span>
            </button>
            <Badge variant="outline" className="hidden whitespace-nowrap sm:inline-flex">
              {credentials?.schedulableCount ?? 0} 个可用
            </Badge>
          </div>
          <div className="flex items-center gap-1 sm:gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={handleMode}
              disabled={isLoadingMode || setLoadBalancingMode.isPending}
              className="hidden sm:inline-flex"
            >
              {loadBalancingData?.mode === 'balanced' ? '均衡负载' : '优先级模式'}
            </Button>
            <Button variant="ghost" size="icon" onClick={handleTheme} title="切换主题">
              {theme === 'dark' ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
            </Button>
            <Button variant="ghost" size="icon" onClick={handleRefresh} title="刷新">
              <RefreshCw className="h-4 w-4" />
            </Button>
            <Button variant="ghost" size="icon" onClick={handleLogout} title="退出">
              <LogOut className="h-4 w-4" />
            </Button>
          </div>
        </div>
        <nav className="mx-auto flex max-w-[1500px] gap-1 overflow-x-auto px-4 pb-3 md:px-6">
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              className={({ isActive }) =>
                cn(
                  'inline-flex h-9 items-center gap-2 rounded-md px-3 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground',
                  isActive && 'bg-primary text-primary-foreground hover:bg-primary hover:text-primary-foreground',
                )
              }
            >
              <item.icon className="h-4 w-4" />
              {item.label}
            </NavLink>
          ))}
        </nav>
      </header>
      <main className="mx-auto max-w-[1500px] space-y-6 px-4 py-6 md:px-6">
        <Outlet />
      </main>
    </div>
  )
}
