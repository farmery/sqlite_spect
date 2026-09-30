import { useEffect, useRef } from 'react'
import { useColResize } from '../hooks/useColResize'
import { useRowEdit } from '../hooks/useRowEdit'
import { useTableData } from '../hooks/useTableData'
import type { ModalRequest } from '../types'
import { CellValue, formatDuration } from './CellValue'

type Props = {
  db: string
  table: string
  onOpenModal: (m: ModalRequest) => void
}

export function TableView({ db, table, onOpenModal }: Props) {
  const data = useTableData(db, table)
  const edit = useRowEdit(data, db, table)
  const resize = useColResize(data.displayCols.length)

  const {
    result, rows, loading, error, banner, setBanner,
    offset,
    displayCols, pkCols, rowKey, canWrite, hasPrev, hasNext,
    deleteRowIdx, setDeleteRowIdx,
    fetchPage, confirmDelete,
  } = data

  const {
    editTarget, editValue, editError, pendingEdit, editInputRef,
    enterEditMode, revertEdit, commitEdit, handleEditKeyDown, onEditChange,
  } = edit

  const { colWidths, onResizeStart } = resize

  // Auto-dismiss banners that set autoClose.
  const bannerTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    if (bannerTimerRef.current) clearTimeout(bannerTimerRef.current)
    if (banner?.autoClose) {
      bannerTimerRef.current = setTimeout(() => setBanner(null), banner.autoClose)
    }
    return () => {
      if (bannerTimerRef.current) clearTimeout(bannerTimerRef.current)
    }
  }, [banner, setBanner])

  return (
    <div className="pane">
      {/* ---- Header ---- */}
      <header className="pane-header">
        <div className="pane-title">
          <span className="pane-icon">▦</span>
          <h2>{table}</h2>
          <span className="pane-sub">
            {result && `${displayCols.length} cols · ${rows.length} rows`}
            {result && result.durationMicros > 0 && (
              <span className="pane-timing"> · query: {formatDuration(result.durationMicros)}</span>
            )}
          </span>
        </div>
        <div className="pane-actions">
          {canWrite && (
            <button className="btn btn-sm" onClick={() => onOpenModal({ kind: 'insert', db, table })}>
              + Add row
            </button>
          )}
          <button className="btn btn-sm" onClick={() => onOpenModal({ kind: 'schema', db, table })}>
            Schema
          </button>
          <button
            className="btn btn-icon"
            disabled={loading}
            title="Refresh"
            onClick={() => {
              if (editTarget && !window.confirm('Discard edit?')) return
              if (editTarget) revertEdit()
              fetchPage(offset)
            }}
          >
            {loading ? '…' : '↻'}
          </button>
        </div>
      </header>

      {/* ---- Banners ---- */}
      {rowKey?.kind === 'none' && (
        <div className="banner banner-info">
          This table has no primary key or rowid — editing is disabled.
        </div>
      )}
      {error && <div className="banner banner-error">{error}</div>}
      {banner && (
        <div className={`banner banner-${banner.kind}`}>
          {banner.msg}
          <button className="btn btn-inline" onClick={() => setBanner(null)}>✕</button>
        </div>
      )}

      {/* ---- Table ---- */}
      <div className="table-wrap">
        <table className="data-table" style={{ tableLayout: colWidths.length ? 'fixed' : 'auto' }}>
          {colWidths.length > 0 && (
            <colgroup>
              <col style={{ width: 48 }} />
              {displayCols.map((c, i) => <col key={c} style={{ width: colWidths[i] ?? 160 }} />)}
              {canWrite && <col style={{ width: 32 }} />}
            </colgroup>
          )}
          <thead>
            <tr>
              <th className="rownum">#</th>
              {displayCols.map((c, i) => (
                <th key={c} style={{ position: 'relative' }}>
                  <span className="th-label">
                    {c}
                    {pkCols.has(c) && <PkGlyph pkCols={pkCols} col={c} />}
                  </span>
                  <div className="col-resizer" onMouseDown={(e) => onResizeStart(e, i)} />
                </th>
              ))}
              {canWrite && <th className="delete-col" />}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, i) => {
              const displayRow = displayCols.map((c) => {
                const ci = result?.columns.indexOf(c) ?? -1
                return ci >= 0 ? row[ci] : null
              })
              return (
                <tr key={offset + i} className={deleteRowIdx === i ? 'row-deleting' : ''}>
                  <td className="rownum">{offset + i + 1}</td>

                  {displayRow.map((cell, j) => {
                    const isEditing = editTarget?.rowIdx === i && editTarget.colIdx === j
                    const isPending = pendingEdit?.rowIdx === i && pendingEdit.colIdx === j
                    const isBlob = typeof cell === 'string' && cell.startsWith('<blob')
                    return (
                      <td
                        key={j}
                        className={isEditing ? 'cell-editing' : ''}
                        onDoubleClick={() => !isBlob && enterEditMode(i, j)}
                        title={isBlob ? 'BLOB values are read-only in v1' : undefined}
                      >
                        {isEditing ? (
                          <>
                            <input
                              ref={editInputRef}
                              className={`cell-input${editError ? ' cell-input-error' : ''}`}
                              value={editValue}
                              onChange={(e) => onEditChange(e.target.value)}
                              onKeyDown={handleEditKeyDown}
                              onBlur={commitEdit}
                            />
                            {editError && <div className="cell-error-tip">{editError}</div>}
                          </>
                        ) : (
                          <>
                            <CellValue value={cell} />
                            {isPending && <span className="cell-spinner">…</span>}
                          </>
                        )}
                      </td>
                    )
                  })}

                  {canWrite && (
                    <td className="delete-col">
                      {deleteRowIdx === i ? (
                        <span className="delete-confirm">
                          <button className="btn btn-xs" onClick={() => setDeleteRowIdx(null)}>Cancel</button>
                          <button className="btn btn-xs btn-danger" onClick={() => confirmDelete(i)}>Delete</button>
                        </span>
                      ) : (
                        <button className="btn-trash" title="Delete row" onClick={() => setDeleteRowIdx(i)}>🗑</button>
                      )}
                    </td>
                  )}
                </tr>
              )
            })}
            {!loading && rows.length === 0 && (
              <tr>
                <td colSpan={displayCols.length + 2} className="empty-row">no rows</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* ---- Pagination ---- */}
      <footer className="pane-footer">
        <button className="btn" disabled={!hasPrev || loading} onClick={() => fetchPage(Math.max(0, offset - 50))}>
          ← Prev
        </button>
        <span className="pager-info">rows {offset + 1}–{offset + rows.length}</span>
        <button className="btn" disabled={!hasNext || loading} onClick={() => fetchPage(offset + 50)}>
          Next →
        </button>
      </footer>
    </div>
  )
}

function PkGlyph({ pkCols, col }: { pkCols: Set<string>; col: string }) {
  const label = pkCols.size === 1
    ? 'Primary key'
    : `Primary key (${Array.from(pkCols).indexOf(col) + 1} of ${pkCols.size})`
  return (
    <svg className="pk-glyph" width="12" height="12" viewBox="0 0 12 12" fill="currentColor" opacity="0.65" aria-label={label}>
      <title>{label}</title>
      <circle cx="4.5" cy="4.5" r="3" stroke="currentColor" strokeWidth="1.5" fill="none" />
      <line x1="6.5" y1="6.5" x2="10" y2="10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      <line x1="8" y1="8.5" x2="10" y2="8.5" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
      <line x1="9" y1="7.5" x2="9" y2="10" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
    </svg>
  )
}
