import { useState, useEffect } from 'react'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { storage } from '@/lib/storage'
import { LoginPage } from '@/components/login-page'
import { AppLayout } from '@/components/app-layout'
import { Toaster } from '@/components/ui/sonner'
import { MonitorPage } from '@/pages/monitor-page'
import { AccountsPage } from '@/pages/accounts-page'
import { StatsPage } from '@/pages/stats-page'
import { RecordsPage } from '@/pages/records-page'
import { ProxiesPage } from '@/pages/proxies-page'
import { SettingsPage } from '@/pages/settings-page'

function App() {
  const [isLoggedIn, setIsLoggedIn] = useState(false)

  useEffect(() => {
    // 检查是否已经有保存的 API Key
    if (storage.getApiKey()) {
      setIsLoggedIn(true)
    }
  }, [])

  const handleLogin = () => {
    setIsLoggedIn(true)
  }

  const handleLogout = () => {
    setIsLoggedIn(false)
  }

  return (
    <>
      {isLoggedIn ? (
        <BrowserRouter basename="/admin">
          <Routes>
            <Route element={<AppLayout onLogout={handleLogout} />}>
              <Route index element={<Navigate to="/monitor" replace />} />
              <Route path="/monitor" element={<MonitorPage />} />
              <Route path="/accounts" element={<AccountsPage />} />
              <Route path="/stats" element={<StatsPage />} />
              <Route path="/records" element={<RecordsPage />} />
              <Route path="/proxies" element={<ProxiesPage />} />
              <Route path="/settings" element={<SettingsPage />} />
              <Route path="*" element={<Navigate to="/monitor" replace />} />
            </Route>
          </Routes>
        </BrowserRouter>
      ) : (
        <LoginPage onLogin={handleLogin} />
      )}
      <Toaster position="top-right" />
    </>
  )
}

export default App
