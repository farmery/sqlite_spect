import type { Cell } from '../types'

export function CellValue({ value }: { value: Cell }) {
  if (value === null || value === undefined) return <span className="cell-null">NULL</span>
  if (typeof value === 'boolean') {
    return <span className={value ? 'cell-bool-true' : 'cell-bool-false'}>{String(value)}</span>
  }
  if (typeof value === 'number') return <span className="cell-num">{value}</span>
  if (typeof value === 'string' && value.startsWith('<blob')) {
    return <span className="cell-null">{value}</span>
  }
  return <span className="cell-text">{String(value)}</span>
}

export function formatDuration(micros: number): string {
  if (micros < 1000) return `${micros}µs`
  if (micros < 1_000_000) return `${(micros / 1000).toFixed(1)}ms`
  return `${(micros / 1_000_000).toFixed(2)}s`
}
