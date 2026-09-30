import { useState, type KeyboardEvent } from 'react'
import { useRpc } from '../RpcContext'
import type { ExecuteResult, QueryResult } from '../types'
import { CellValue, formatDuration } from './CellValue'

type Props = { db: string }

type RunOutcome =
  | { kind: 'query'; result: QueryResult }
  | { kind: 'execute'; result: ExecuteResult }
  | { kind: 'error'; message: string }

// Heuristic: treat anything starting with SELECT/WITH/PRAGMA/EXPLAIN as a read.
function isReadQuery(sql: string): boolean {
  const t = sql.trim().toUpperCase()
  return (
    t.startsWith('SELECT') ||
    t.startsWith('WITH') ||
    t.startsWith('PRAGMA') ||
    t.startsWith('EXPLAIN')
  )
}

export function SqlRunner({ db }: Props) {
  const { client } = useRpc()
  const [sql, setSql] = useState(`SELECT name FROM sqlite_master WHERE type = 'table';`)
  const [outcome, setOutcome] = useState<RunOutcome | null>(null)
  const [running, setRunning] = useState(false)

  async function run() {
    if (!client || !sql.trim()) return
    setRunning(true)
    setOutcome(null)
    try {
      if (isReadQuery(sql)) {
        const result = await client.call<QueryResult>('db.query', { db, sql, limit: 200 })
        setOutcome({ kind: 'query', result })
      } else {
        const result = await client.call<ExecuteResult>('db.execute', { db, sql })
        setOutcome({ kind: 'execute', result })
      }
    } catch (e) {
      const msg = (e as { message?: string }).message ?? 'query failed'
      setOutcome({ kind: 'error', message: msg })
    } finally {
      setRunning(false)
    }
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault()
      run()
    }
  }

  return (
    <div className="pane">
      <header className="pane-header">
        <div className="pane-title">
          <span className="pane-icon">›_</span>
          <h2>SQL Runner</h2>
          <span className="pane-sub">on {db}</span>
        </div>
        <button className="btn btn-primary" onClick={run} disabled={running || !sql.trim()}>
          {running ? 'Running…' : 'Run  ⌘↵'}
        </button>
      </header>

      <div className="sql-editor">
        <textarea
          value={sql}
          onChange={(e) => setSql(e.target.value)}
          onKeyDown={onKeyDown}
          spellCheck={false}
          placeholder="SELECT * FROM …"
        />
      </div>

      {outcome?.kind === 'error' && (
        <div className="banner banner-error">{outcome.message}</div>
      )}

      {outcome?.kind === 'execute' && (
        <div className="banner banner-success">
          <span>
            {outcome.result.rowsAffected} row{outcome.result.rowsAffected === 1 ? '' : 's'} affected
            {outcome.result.lastInsertRowid
              ? ` · last insert rowid ${outcome.result.lastInsertRowid}`
              : ''}
          </span>
          <span className="pane-timing">
            {formatDuration(outcome.result.durationMicros)}
          </span>
        </div>
      )}

      {outcome?.kind === 'query' && <ResultsTable result={outcome.result} />}
    </div>
  )
}

function ResultsTable({ result }: { result: QueryResult }) {
  return (
    <>
      <div className="results-meta">
        <span>
          {result.columns.length} col{result.columns.length === 1 ? '' : 's'} ·{' '}
          {result.rows.length} row{result.rows.length === 1 ? '' : 's'}
          {result.hasMore && ' · truncated'}
        </span>
        <span className="pane-timing">{formatDuration(result.durationMicros)}</span>
      </div>
      <div className="table-wrap">
        <table className="data-table">
          <thead>
            <tr>
              <th className="rownum">#</th>
              {result.columns.map((c) => (
                <th key={c}>{c}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {result.rows.map((row, i) => (
              <tr key={i}>
                <td className="rownum">{i + 1}</td>
                {row.map((cell, j) => (
                  <td key={j}>
                    <CellValue value={cell} />
                  </td>
                ))}
              </tr>
            ))}
            {result.rows.length === 0 && (
              <tr>
                <td colSpan={result.columns.length + 1} className="empty-row">
                  no rows
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </>
  )
}
