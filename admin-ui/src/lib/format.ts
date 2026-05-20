export function formatNumber(value?: number | null) {
  return new Intl.NumberFormat('zh-CN').format(value ?? 0)
}

export function formatDuration(value?: number | null) {
  const ms = value ?? 0
  if (ms < 1000) return `${Math.round(ms)}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

export function formatTime(value?: string | null) {
  if (!value) return '未记录'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString('zh-CN')
}

export function formatRelativeTime(value?: string | null) {
  if (!value) return '从未使用'
  const date = new Date(value)
  const diff = Date.now() - date.getTime()
  if (Number.isNaN(diff)) return value
  if (diff < 60_000) return '刚刚'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`
  return `${Math.floor(diff / 86_400_000)} 天前`
}

export function percent(part?: number | null, total?: number | null) {
  if (!total) return '0.0%'
  return `${(((part ?? 0) / total) * 100).toFixed(1)}%`
}
