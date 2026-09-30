import { useEffect, useState } from 'react'
import { useRpc } from '../../RpcContext'
import type { Schema, TableInfo } from '../../types'
import { Modal, ModalBody } from './Modal'

type Props = {
  db: string
  table: string
  onClose: () => void
}

export function SchemaModal({ db, table, onClose }: Props) {
  const { client } = useRpc()
  const [info, setInfo] = useState<TableInfo | null>(null)
  const [schema, setSchema] = useState<Schema | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [showCreate, setShowCreate] = useState(false)

  useEffect(() => {
    if (!client) return
    Promise.all([
      client.call<TableInfo>('db.tableInfo', { db, table }),
      client.call<Schema>('db.schema', { db }),
    ])
      .then(([ti, s]) => { setInfo(ti); setSchema(s) })
      .catch((e) => setError(e.message ?? 'failed'))
      .finally(() => setLoading(false))
  }, [client, db, table])

  const createSql = schema?.tables.find((t) => t.name === table)?.sql
    ?? schema?.views.find((v) => v.name === table)?.sql
    ?? null

  const isView = schema?.views.some((v) => v.name === table) ?? false

  // Parse trigger timing/event from CREATE TRIGGER SQL
  function parseTrigger(sql: string | null): { timing: string; event: string } | null {
    if (!sql) return null
    const m = sql.match(/CREATE\s+TRIGGER\s+\S+\s+(BEFORE|AFTER|INSTEAD\s+OF)\s+(INSERT|UPDATE|DELETE)/i)
    if (!m) return null
    return { timing: m[1].toUpperCase(), event: m[2].toUpperCase() }
  }

  const triggers = schema?.triggers.filter((t) => {
    // Match triggers that reference this table
    return t.sql?.includes(table) || false
  }) ?? []

  return (
    <Modal title={table} size="lg" onClose={onClose}>
      <ModalBody>
        {loading && <p className="modal-loading">Loading…</p>}
        {error && <p className="modal-error">{error}</p>}

        {info && (
          <>
            <div className="schema-subtitle">
              <span className="badge">{isView ? 'View' : 'Table'}</span>
            </div>

            {/* Columns */}
            <section className="schema-section">
              <h3 className="schema-section-title">Columns</h3>
              <table className="schema-table">
                <thead>
                  <tr>
                    <th>#</th>
                    <th>Name</th>
                    <th>Type</th>
                    <th>Nullable</th>
                    <th>Default</th>
                  </tr>
                </thead>
                <tbody>
                  {info.columns.map((col) => (
                    <tr key={col.cid}>
                      <td className="schema-num">{col.cid}</td>
                      <td>
                        {col.name}
                        {col.pk > 0 && <span className="badge badge-pk">PK</span>}
                        {col.notNull && <span className="badge badge-nn">NN</span>}
                      </td>
                      <td className="mono">{col.type || '—'}</td>
                      <td>{col.notNull ? 'No' : 'Yes'}</td>
                      <td className="mono">{col.default ?? '—'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>

            {/* Indexes */}
            {info.indexes.length > 0 && (
              <section className="schema-section">
                <h3 className="schema-section-title">Indexes</h3>
                {info.indexes.map((idx) => (
                  <div key={idx.name} className="schema-row">
                    <span className="schema-row-name">
                      {idx.name}
                      {idx.unique && <span className="badge badge-unique">UNIQUE</span>}
                    </span>
                    <span className="schema-row-detail">
                      {idx.columns.map((c) => c.name).join(', ')}
                    </span>
                  </div>
                ))}
              </section>
            )}

            {/* Foreign keys */}
            {info.foreignKeys.length > 0 && (
              <section className="schema-section">
                <h3 className="schema-section-title">Foreign Keys</h3>
                {info.foreignKeys.map((fk) => (
                  <div key={fk.id} className="schema-row">
                    <span className="schema-row-name">
                      {fk.from} → {fk.table}.{fk.to}
                    </span>
                    <span className="schema-row-detail">
                      {fk.onDelete !== 'NO ACTION' && (
                        <span className="badge">ON DELETE {fk.onDelete}</span>
                      )}
                      {fk.onUpdate !== 'NO ACTION' && (
                        <span className="badge">ON UPDATE {fk.onUpdate}</span>
                      )}
                    </span>
                  </div>
                ))}
              </section>
            )}

            {/* Triggers */}
            {triggers.length > 0 && (
              <section className="schema-section">
                <h3 className="schema-section-title">Triggers</h3>
                {triggers.map((t) => {
                  const parsed = parseTrigger(t.sql)
                  return (
                    <div key={t.name} className="schema-row">
                      <span className="schema-row-name">{t.name}</span>
                      {parsed && (
                        <span className="schema-row-detail">
                          <span className="badge">{parsed.timing}</span>
                          <span className="badge">{parsed.event}</span>
                        </span>
                      )}
                    </div>
                  )
                })}
              </section>
            )}

            {/* Create SQL */}
            {createSql && (
              <section className="schema-section">
                <button
                  className="schema-toggle"
                  onClick={() => setShowCreate((v) => !v)}
                >
                  {showCreate ? '▼' : '▶'} Create SQL
                </button>
                {showCreate && (
                  <pre className="schema-sql">{createSql}</pre>
                )}
              </section>
            )}
          </>
        )}
      </ModalBody>
    </Modal>
  )
}
