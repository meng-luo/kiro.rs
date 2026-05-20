import React from 'react'
import ReactDOM from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import App from './App'
import { storage } from './lib/storage'
import './index.css'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5000,
      refetchOnWindowFocus: false,
    },
  },
})

const savedTheme = storage.getTheme()
const prefersDark = window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false
document.documentElement.classList.toggle('dark', savedTheme === 'dark' || (savedTheme === 'system' && prefersDark))

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
)
