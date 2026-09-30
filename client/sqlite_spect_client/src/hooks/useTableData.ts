import { useCallback, useEffect, useRef, useState } from 'react'
import { useRpc } from '../RpcContext'
import type { Cell, MutationEvent, Row, RowKey, TableInfo, TableRowsResult } from '../types'

const PAGE_SIZE = 50

export type Banner = { kind: 'info' | 'success' | 'error'; msg: string; autoClose?: number }

export function useTableData(db: string, table: string) {
  const { client } = useRpc()

  const [result, setResult] = useState<TableRowsResult | null>(null)
  const [tableInfo, setTableInfo] = useState<TableInfo | null>(null)
  const [rows, setRows] = useState<Row[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [banner, setBanner] = useState<Banner | null>(null)
  const [offset, setOffset] = useState(0)
  const [deleteRowIdx, setDeleteRowIdx] = useState<number | null>(null)

  // ---- Fetching ----

  const fetchPage = useCallback(
    (nextOffset: number, successMsg?: string) => {
      if (!client) return
      setLoading(true)
      setError(null)
      setBanner(null)
      Promise.all([
        client.call<TableRowsResult>('db.tableRows', { db, table, limit: PAGE_SIZE, offset: nextOffset }),
        client.call<TableInfo>('db.tableInfo', { db, table }),
      ])
        .then(([r, ti]) => {
          setResult(r)
          setRows(r.rows)
          setTableInfo(ti)
          setOffset(nextOffset)
          if (successMsg) setBanner({ kind: 'info', msg: successMsg, autoClose: 2500 })
        })
        .catch((e) => setError(e.message ?? 'query failed'))
        .finally(() => setLoading(false))
    },
    [client, db, table]
  )

  useEffect(() => {
    setResult(null)
    setRows([])
    setOffset(0)
    fetchPage(0)
  }, [db, table, fetchPage])

  // Auto-refresh when the database changes externally.
  // Debounced so rapid back-to-back mutations trigger a single fetch.
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    if (!client) return
    const unsub = client.on('db.mutated', (params) => {
      const p = params as MutationEvent
      if (p.db !== db) return
      if (refreshTimer.current) clearTimeout(refreshTimer.current)
      refreshTimer.current = setTimeout(() => {
        fetchPage(offset, 'Database updated')
      }, 300)
    })
    return () => {
      unsub()
      if (refreshTimer.current) clearTimeout(refreshTimer.current)
    }
  }, [client, db, offset, fetchPage])

  // ---- Derived ----

  const rowKey: RowKey | null = result?.rowKey ?? null
  const idAlias = rowKey?.kind === 'rowid' ? rowKey.alias : null
  const displayCols = result ? result.columns.filter((c) => c !== idAlias) : []
  const pkCols = new Set<string>(rowKey?.kind === 'pk' ? rowKey.cols : [])
  const canWrite = result !== null && rowKey?.kind !== 'none'
  const hasPrev = offset > 0
  const hasNext = result?.hasMore ?? false

  // ---- Row identity ----

  function getRowIdentity(rowIdx: number): Record<string, Cell> {
    if (!result) return {}
    const fullRow = rows[rowIdx]
    if (rowKey?.kind === 'pk') {
      const identity: Record<string, Cell> = {}
      rowKey.cols.forEach((c) => {
        const ci = result.columns.indexOf(c)
        if (ci >= 0) identity[c] = fullRow[ci]
      })
      return identity
    }
    if (rowKey?.kind === 'rowid') {
      const ci = result.columns.indexOf(rowKey.alias)
      return { [rowKey.alias]: ci >= 0 ? fullRow[ci] : null }
    }
    return {}
  }

  // Maps a display-column index to its index in the full (un-filtered) columns array.
  function fullColIdx(displayIdx: number): number {
    if (!result) return displayIdx
    return result.columns.indexOf(displayCols[displayIdx])
  }

  // ---- Delete ----

  function confirmDelete(rowIdx: number) {
    if (!client || !result) return
    const identity = getRowIdentity(rowIdx)
    const originalRow = rows[rowIdx]

    setRows((prev) => prev.filter((_, i) => i !== rowIdx))
    setDeleteRowIdx(null)

    client
      .call<{ rowsAffected: number }>('db.deleteRow', { db, table, rowKey: identity })
      .then((r) => {
        if (r.rowsAffected === 0) setBanner({ kind: 'info', msg: 'Row no longer exists — refreshing.' })
        fetchPage(offset)
      })
      .catch((e) => {
        setRows((prev) => {
          const next = [...prev]
          next.splice(rowIdx, 0, originalRow)
          return next
        })
        setBanner({ kind: 'error', msg: e.message ?? 'Delete failed' })
      })
  }

  return {
    // raw state
    result, tableInfo, rows, setRows, loading, error,
    banner, setBanner,
    offset,
    deleteRowIdx, setDeleteRowIdx,
    // derived
    displayCols, pkCols, rowKey, canWrite, hasPrev, hasNext,
    // actions
    fetchPage, getRowIdentity, fullColIdx, confirmDelete,
  }
}
