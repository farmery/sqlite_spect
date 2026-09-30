import { useEffect, useState } from 'react'
import { useRpc } from '../../RpcContext'
import type { Cell, TableColumn, TableInfo } from '../../types'
import { Modal, ModalBody, ModalFooter } from './Modal'

type Props = {
  db: string
  table: string
  onClose: () => void
  onSuccess: () => void
}

type FieldState = {
  value: string
  isNull: boolean
  error: string | null
}

export function InsertModal({ db, table, onClose, onSuccess }: Props) {
  const { client } = useRpc()
  const [info, setInfo] = useState<TableInfo | null>(null)
  const [loading, setLoading] = useState(true)
  const [submitting, setSubmitting] = useState(false)
  const [fields, setFields] = useState<Record<string, FieldState>>({})
  const [footerError, setFooterError] = useState<string | null>(null)

  useEffect(() => {
    if (!client) return
    client
      .call<TableInfo>('db.tableInfo', { db, table })
      .then((ti) => {
        setInfo(ti)
        const initial: Record<string, FieldState> = {}
        ti.columns.forEach((col) => {
          initial[col.name] = { value: col.default ?? '', isNull: false, error: null }
        })
        setFields(initial)
      })
      .catch((e) => setFooterError(e.message ?? 'failed'))
      .finally(() => setLoading(false))
  }, [client, db, table])

  function setField(name: string, patch: Partial<FieldState>) {
    setFields((prev) => ({
      ...prev,
      [name]: { ...prev[name], ...patch },
    }))
  }

  function isAutoincrement(col: TableColumn): boolean {
    return (
      col.pk === 1 &&
      info!.columns.filter((c) => c.pk > 0).length === 1 &&
      col.type.toUpperCase().includes('INT')
    )
  }

  function validate(): boolean {
    if (!info) return false
    let valid = true
    const updated = { ...fields }
    info.columns.forEach((col) => {
      const f = updated[col.name]
      // NOT NULL without default and not NULL-toggled and not empty auto-increment
      if (col.notNull && !col.default && !f.isNull && f.value.trim() === '' && !isAutoincrement(col)) {
        updated[col.name] = { ...f, error: 'Required' }
        valid = false
      } else {
        updated[col.name] = { ...f, error: null }
      }
    })
    setFields(updated)
    return valid
  }

  function buildValues(): Record<string, Cell> {
    const values: Record<string, Cell> = {}
    info!.columns.forEach((col) => {
      const f = fields[col.name]
      if (!f) return
      // Skip auto-increment if left blank
      if (isAutoincrement(col) && f.value.trim() === '' && !f.isNull) return
      // BLOB columns skipped
      if (col.type.toUpperCase().includes('BLOB')) return

      if (f.isNull) {
        values[col.name] = null
      } else {
        values[col.name] = coerce(f.value, col.type)
      }
    })
    return values
  }

  function coerce(s: string, colType: string): Cell {
    const upper = s.trim().toUpperCase()
    if (upper === 'NULL' || s === '') return null
    const t = colType.toUpperCase()
    if (t.includes('INT')) {
      const n = parseInt(s, 10)
      if (!isNaN(n) && String(n) === s.trim()) return n
      return s
    }
    if (t.includes('REAL') || t.includes('FLOAT') || t.includes('DOUBLE')) {
      const n = Number(s)
      if (!isNaN(n)) return n
      return s
    }
    return s
  }

  function submit() {
    if (!validate()) return
    setSubmitting(true)
    setFooterError(null)
    const values = buildValues()
    client!
      .call<{ rowsAffected: number; lastInsertRowid?: number }>('db.insertRow', {
        db,
        table,
        values,
      })
      .then(() => {
        onSuccess()
      })
      .catch((e) => setFooterError(e.message ?? 'Insert failed'))
      .finally(() => setSubmitting(false))
  }

  const isValid =
    info !== null &&
    info.columns.every((col) => {
      const f = fields[col.name]
      return !f?.error
    })

  return (
    <Modal title={`Insert row into ${table}`} size="md" onClose={onClose}>
      <ModalBody>
        {loading && <p className="modal-loading">Loading schema…</p>}
        {info && (
          <div className="insert-form">
            {info.columns.map((col) => {
              const f = fields[col.name] ?? { value: '', isNull: false, error: null }
              const isBlob = col.type.toUpperCase().includes('BLOB')
              const isAuto = isAutoincrement(col)
              const canNull = !col.notNull || !!col.default
              return (
                <div key={col.name} className="insert-field">
                  <div className="insert-field-header">
                    <span className="insert-field-name">{col.name}</span>
                    <span className="insert-field-badges">
                      {col.pk > 0 && <span className="badge badge-pk">PK</span>}
                      {col.notNull && <span className="badge badge-nn">NN</span>}
                    </span>
                    <span className="insert-field-type">{col.type || 'TEXT'}</span>
                  </div>
                  {isAuto && <p className="insert-hint">auto increment — leave blank to auto-generate</p>}
                  <div className="insert-field-row">
                    {isBlob ? (
                      <input
                        className="insert-input"
                        disabled
                        placeholder="binary — unsupported"
                      />
                    ) : (
                      <input
                        className={`insert-input${f.isNull ? ' input-null' : ''}${f.error ? ' input-error' : ''}`}
                        disabled={f.isNull}
                        value={f.isNull ? '' : f.value}
                        placeholder={col.default ?? (f.isNull ? 'NULL' : '')}
                        onChange={(e) => setField(col.name, { value: e.target.value })}
                        onKeyDown={(e) => { if (e.key === 'Enter') submit() }}
                      />
                    )}
                    {canNull && !isBlob && (
                      <button
                        className={`btn btn-xs null-toggle${f.isNull ? ' active' : ''}`}
                        type="button"
                        title="Send NULL"
                        onClick={() => setField(col.name, { isNull: !f.isNull, error: null })}
                      >
                        NULL
                      </button>
                    )}
                  </div>
                  {f.error && <p className="insert-error">{f.error}</p>}
                </div>
              )
            })}
          </div>
        )}
      </ModalBody>
      <ModalFooter>
        {footerError && <span className="modal-footer-error">{footerError}</span>}
        <button className="btn" onClick={onClose}>Cancel</button>
        <button
          className="btn btn-primary"
          disabled={!isValid || submitting || loading}
          onClick={submit}
        >
          {submitting ? 'Inserting…' : 'Insert'}
        </button>
      </ModalFooter>
    </Modal>
  )
}
