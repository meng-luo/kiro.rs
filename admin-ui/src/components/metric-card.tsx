import type { LucideIcon } from 'lucide-react'
import { Card, CardContent } from '@/components/ui/card'
import { cn } from '@/lib/utils'

interface MetricCardProps {
  label: string
  value: string | number
  hint?: string
  icon?: LucideIcon
  tone?: string
}

export function MetricCard({ label, value, hint, icon: Icon, tone }: MetricCardProps) {
  return (
    <Card className="rounded-md">
      <CardContent className="p-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="truncate text-xs font-medium text-muted-foreground">{label}</div>
            <div className="mt-2 truncate text-2xl font-semibold">{value}</div>
            {hint ? <div className="mt-1 truncate text-xs text-muted-foreground">{hint}</div> : null}
          </div>
          {Icon ? (
            <div className={cn('rounded-md bg-muted p-2 text-muted-foreground', tone)}>
              <Icon className="h-4 w-4" />
            </div>
          ) : null}
        </div>
      </CardContent>
    </Card>
  )
}
