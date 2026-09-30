import { useEffect, useRef, useState } from 'react'
import { useRpc } from '../RpcContext'
import type { Cell } from '../types'
import type { useTableData } from './useTableData'

type TableData = ReturnType<typeof useTableData>

type EditTarget = { rowIdx: number; colIdx: number; originalValue: Cell }
type PendingEdit = { rowIdx: number; colIdx: number }

export function useRowEdit(data: TableData, db: string, table: string) {
  const { client } = useRpc()

  const [editTarget, setEditTarget] = useState<EditTarget | null>(null)
  const [editValue, setEditValue] = useState('')
  const [editError, setEditError] = useState<string | null>(null)
  const [pendingEdit, setPendingEdit] = useState<PendingEdit | null>(null)
  const editInputRef = useRef<HTMLInputElement>(null)

  const { rows, setRows, setBanner, fetchPage, offset, displayCols, tableInfo, canWrite, fullColIdx, getRowIdentity } = data

  // Focus + select-all whenever a cell enters edit mode.
  useEffect(() => {
    if (editTarget !== null) {
      editInputRef.current?.focus()
      editInputRef.current?.select()
    }
  }, [editTarget])

  function enterEditMode(rowIdx: number, displayIdx: number) {
    if (!canWrite) return
    const fci = fullColIdx(displayIdx)
    const cell = rows[rowIdx][fci]
    if (typeof cell === 'string' && cell.startsWith('<blob')) return // BLOBs are read-only
    setEditTarget({ rowIdx, colIdx: displayIdx, originalValue: cell })
    setEditValue(cell === null ? '' : String(cell))
    setEditError(null)
  }

  function revertEdit() {
    setEditTarget(null)
    setEditError(null)
  }

  function commitEdit() {
    if (!editTarget || !client) return
    const { rowIdx, colIdx, originalValue } = editTarget
    const fci = fullColIdx(colIdx)
    const colName = displayCols[colIdx]
    const colMeta = tableInfo?.columns.find((c) => c.name === colName)

    const validationError = validateEdit(editValue, colMeta?.type ?? '')
    if (validationError) {
      setEditError(validationError)
      editInputRef.current?.focus()
      return
    }
    setEditError(null)

    const newValue = coerceValue(editValue, colMeta?.type ?? '')
    if (newValue === originalValue) {
      setEditTarget(null)
      return
    }

    // Optimistic update
    setRows((prev) => {
      const next = prev.map((r) => [...r])
      next[rowIdx][fci] = newValue
      return next
    })
    setEditTarget(null)
    setPendingEdit({ rowIdx, colIdx })

    const identity = getRowIdentity(rowIdx)
    client
      .call<{ rowsAffected: number }>('db.updateRow', {
        db, table, rowKey: identity, updates: { [colName]: newValue },
      })
      .then((r) => {
        if (r.rowsAffected === 0) {
          setRows((prev) => {
            const next = prev.map((r2) => [...r2])
            next[rowIdx][fci] = originalValue
            return next
          })
          setBanner({ kind: 'info', msg: 'Row no longer exists — refreshing.' })
          fetchPage(offset)
        }
      })
      .catch((e) => {
        setRows((prev) => {
          const next = prev.map((r2) => [...r2])
          next[rowIdx][fci] = originalValue
          return next
        })
        setBanner({ kind: 'error', msg: e.message ?? 'Update failed' })
      })
      .finally(() => setPendingEdit(null))
  }

  function handleEditKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Escape') {
      revertEdit()
    } else if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      commitEdit()
    } else if (e.key === 'Tab') {
      e.preventDefault()
      commitEdit()
      if (!editTarget) return
      const dir = e.shiftKey ? -1 : 1
      const nextCol = editTarget.colIdx + dir
      if (nextCol >= 0 && nextCol < displayCols.length) {
        setTimeout(() => enterEditMode(editTarget.rowIdx, nextCol), 0)
      }
    }
  }

  function onEditChange(value: string) {
    setEditValue(value)
    setEditError(null)
  }

  return {
    editTarget, editValue, editError, pendingEdit, editInputRef,
    enterEditMode, revertEdit, commitEdit, handleEditKeyDown, onEditChange,
  }
}

// ---- Helpers ----

function validateEdit(s: string, colType: string): string | null {
  if (s === '' || s.trim().toUpperCase() === 'NULL') return null
  const t = colType.toUpperCase()
  if (t.includes('INT')) {
    const trimmed = s.trim()
    const n = parseInt(trimmed, 10)
    if (isNaN(n) || String(n) !== trimmed) return `Expected an integer (got "${s}")`
  } else if (t.includes('REAL') || t.includes('FLOAT') || t.includes('DOUBLE') || t.includes('NUMERIC')) {
    if (isNaN(Number(s.trim()))) return `Expected a number (got "${s}")`
  }
  return null
}

function coerceValue(s: string, colType: string): Cell {
  if (s === '' || s.trim().toUpperCase() === 'NULL') return null
  const t = colType.toUpperCase()
  if (t.includes('INT')) return parseInt(s.trim(), 10)
  if (t.includes('REAL') || t.includes('FLOAT') || t.includes('DOUBLE') || t.includes('NUMERIC')) return Number(s.trim())
  return s
}
