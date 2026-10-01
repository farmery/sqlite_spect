import { useEffect, useRef, useState } from 'react'
import { useRpc } from '../RpcContext'
import { EDITING_ENABLED } from '../editing'
import type { DbInfo, ModalRequest, MutationEvent, Schema, Selection } from '../types'

type Props = {
  selection: Selection | null
  onSelect: (s: Selection) => void
  onOpenModal: (m: ModalRequest) => void
}

type ExpandedState = {
  [dbId: string]: {
    schema: Schema | null
    loading: boolean
    error: string | null
  }
}

type MenuState = { db: string; table: string } | null

export function Sidebar({ selection, onSelect, onOpenModal }: Props) {
  const { client, state } = useRpc()
  const [dbs, setDbs] = useState<DbInfo[] | null>(null)
  const [expanded, setExpanded] = useState<ExpandedState>({})
  const [activeDbs, setActiveDbs] = useState<Set<string>>(new Set())
  const [openMenu, setOpenMenu] = useState<MenuState>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  // Fetch DB list on connect.
  useEffect(() => {
    if (!client || state !== 'open') return
    client.call<DbInfo[]>('db.list').then((list) => {
      setDbs(list)
      if (list.length === 1) loadSchema(list[0].id)
    })
  }, [client, state])

  // Subscribe to every DB for live indicators.
  useEffect(() => {
    if (!client || state !== 'open' || !dbs) return
    const subIds: string[] = []
    dbs.forEach((db) => {
      client
        .call<{ subscriptionId: string }>('db.subscribe', { db: db.id })
        .then((r) => subIds.push(r.subscriptionId))
        .catch(() => {})
    })
    const unsub = client.on('db.mutated', (params) => {
      const p = params as MutationEvent
      setActiveDbs((prev) => new Set(prev).add(p.db))
    })
    return () => {
      unsub()
      subIds.forEach((id) =>
        client.call('db.unsubscribe', { subscriptionId: id }).catch(() => {})
      )
    }
  }, [client, state, dbs])

  // Close menu on outside click
  useEffect(() => {
    function handler(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpenMenu(null)
      }
    }
    if (openMenu) document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [openMenu])

  function loadSchema(dbId: string) {
    setExpanded((prev) => ({
      ...prev,
      [dbId]: { schema: null, loading: true, error: null },
    }))
    client!
      .call<Schema>('db.schema', { db: dbId })
      .then((schema) =>
        setExpanded((prev) => ({
          ...prev,
          [dbId]: { schema, loading: false, error: null },
        }))
      )
      .catch((e) =>
        setExpanded((prev) => ({
          ...prev,
          [dbId]: { schema: null, loading: false, error: e.message ?? 'failed' },
        }))
      )
  }

  function toggle(dbId: string) {
    if (expanded[dbId]) {
      setExpanded((prev) => {
        const next = { ...prev }
        delete next[dbId]
        return next
      })
    } else {
      loadSchema(dbId)
    }
  }

  function handleTableClick(dbId: string, tableName: string) {
    setActiveDbs((prev) => {
      const next = new Set(prev)
      next.delete(dbId)
      return next
    })
    onSelect({ kind: 'table', db: dbId, table: tableName })
  }

  function isWritable(db: DbInfo) {
    return db.capabilities.includes('write')
  }

  return (
    <aside className="sidebar">
      <header className="sidebar-header">
        <h1 className="app-title">sqlite spect</h1>
        <StatusDot state={state} />
      </header>

      <div className="sidebar-body">
        {dbs === null && state === 'open' && (
          <p className="sidebar-empty">loading…</p>
        )}
        {dbs !== null && dbs.length === 0 && (
          <p className="sidebar-empty">(no databases)</p>
        )}
        {dbs?.map((db) => {
          const exp = expanded[db.id]
          const isOpen = !!exp
          const hasActivity = activeDbs.has(db.id)
          return (
            <div key={db.id} className="db-node">
              <button
                className="db-row"
                onClick={() => toggle(db.id)}
              >
                <span className="caret">{isOpen ? '▼' : '▶'}</span>
                <span className="db-name">{db.id}</span>
                {hasActivity && <span className="live-dot" title="Unread changes" />}
              </button>
              {isOpen && (
                <div className="table-list">
                  {exp.loading && <p className="sidebar-empty small">loading…</p>}
                  {exp.error && <p className="sidebar-error">{exp.error}</p>}
                  {exp.schema?.tables.length === 0 && (
                    <p className="sidebar-empty small">(no tables)</p>
                  )}
                  {exp.schema?.tables.map((t) => {
                    const isSelected =
                      selection?.kind === 'table' &&
                      selection.db === db.id &&
                      selection.table === t.name
                    const menuOpen =
                      openMenu?.db === db.id && openMenu.table === t.name
                    return (
                      <div key={t.name} className="table-row-wrap">
                        <button
                          className={`table-row ${isSelected ? 'selected' : ''}`}
                          onClick={() => handleTableClick(db.id, t.name)}
                        >
                          {t.name}
                        </button>
                        <div className="table-row-menu-anchor" ref={menuOpen ? menuRef : null}>
                          <button
                            className="table-menu-btn"
                            title="Table actions"
                            onClick={(e) => {
                              e.stopPropagation()
                              setOpenMenu(menuOpen ? null : { db: db.id, table: t.name })
                            }}
                          >
                            ⋯
                          </button>
                          {menuOpen && (
                            <div className="table-context-menu">
                              <button
                                className="menu-item"
                                onClick={() => {
                                  setOpenMenu(null)
                                  onOpenModal({ kind: 'schema', db: db.id, table: t.name })
                                }}
                              >
                                View schema
                              </button>
                              {isWritable(db) && (
                                <>
                                  <button
                                    className="menu-item"
                                    disabled={!EDITING_ENABLED}
                                    title={EDITING_ENABLED ? undefined : 'Work in progress — editing is temporarily disabled'}
                                    onClick={() => {
                                      setOpenMenu(null)
                                      onOpenModal({ kind: 'insert', db: db.id, table: t.name })
                                    }}
                                  >
                                    Add row
                                  </button>
                                  <button
                                    className="menu-item menu-item-danger"
                                    disabled={!EDITING_ENABLED}
                                    title={EDITING_ENABLED ? undefined : 'Work in progress — editing is temporarily disabled'}
                                    onClick={() => {
                                      setOpenMenu(null)
                                      onOpenModal({ kind: 'clear', db: db.id, table: t.name })
                                    }}
                                  >
                                    Clear table…
                                  </button>
                                </>
                              )}
                            </div>
                          )}
                        </div>
                      </div>
                    )
                  })}

                  {/* Views section */}
                  {exp.schema?.views && exp.schema.views.length > 0 && (
                    <>
                      <p className="sidebar-section-label">Views</p>
                      {exp.schema.views.map((v) => {
                        const isSelected =
                          selection?.kind === 'table' &&
                          selection.db === db.id &&
                          selection.table === v.name
                        const menuOpen =
                          openMenu?.db === db.id && openMenu.table === v.name
                        return (
                          <div key={v.name} className="table-row-wrap">
                            <button
                              className={`table-row ${isSelected ? 'selected' : ''}`}
                              onClick={() => handleTableClick(db.id, v.name)}
                            >
                              {v.name}
                            </button>
                            <div className="table-row-menu-anchor" ref={menuOpen ? menuRef : null}>
                              <button
                                className="table-menu-btn"
                                onClick={(e) => {
                                  e.stopPropagation()
                                  setOpenMenu(menuOpen ? null : { db: db.id, table: v.name })
                                }}
                              >
                                ⋯
                              </button>
                              {menuOpen && (
                                <div className="table-context-menu">
                                  <button
                                    className="menu-item"
                                    onClick={() => {
                                      setOpenMenu(null)
                                      onOpenModal({ kind: 'schema', db: db.id, table: v.name })
                                    }}
                                  >
                                    View schema
                                  </button>
                                </div>
                              )}
                            </div>
                          </div>
                        )
                      })}
                    </>
                  )}

                  {exp.schema && (
                    <button
                      className={`sql-runner-btn ${
                        selection?.kind === 'sql' && selection.db === db.id ? 'selected' : ''
                      }`}
                      onClick={() => onSelect({ kind: 'sql', db: db.id })}
                    >
                      <span className="sql-runner-icon">{'</>'}</span>
                      SQL Runner
                    </button>
                  )}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </aside>
  )
}

function StatusDot({ state }: { state: string }) {
  const label =
    state === 'open'
      ? 'connected'
      : state === 'connecting'
      ? 'connecting'
      : state === 'closed'
      ? 'disconnected'
      : 'error'
  return (
    <span className={`status-dot status-${state}`} title={label}>
      <span className="dot" />
    </span>
  )
}
