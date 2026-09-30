import { useEffect, useState } from 'react'
import { useRpc } from '../../RpcContext'
import { Modal, ModalBody, ModalFooter } from './Modal'

type Props = {
  db: string
  table: string
  onClose: () => void
  onSuccess: () => void
}

export function ClearConfirmModal({ db, table, onClose, onSuccess }: Props) {
  const { client } = useRpc()
  const [rowCount, setRowCount] = useState<number | null>(null)
  const [confirmInput, setConfirmInput] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [footerError, setFooterError] = useState<string | null>(null)

  useEffect(() => {
    if (!client) return
    client
      .call<{ columns: string[]; rows: (string | number | boolean | null)[][] }>(
        'db.query',
        { db, sql: `SELECT COUNT(*) FROM ${JSON.stringify(table)}` }
      )
      .then((r) => {
        const count = r.rows[0]?.[0]
        setRowCount(typeof count === 'number' ? count : null)
      })
      .catch(() => setRowCount(null))
  }, [client, db, table])

  function submit() {
    if (confirmInput !== table) return
    setSubmitting(true)
    setFooterError(null)
    client!
      .call<{ rowsAffected: number }>('db.clearTable', {
        db,
        table,
        confirm: confirmInput,
      })
      .then(() => onSuccess())
      .catch((e) => setFooterError(e.message ?? 'Clear failed'))
      .finally(() => setSubmitting(false))
  }

  const canSubmit = confirmInput === table && !submitting

  return (
    <Modal title={`Clear ${table}`} size="sm" onClose={onClose}>
      <ModalBody>
        <p className="clear-warning">
          This will permanently delete all rows in <strong>"{table}"</strong>. This cannot be undone.
        </p>
        {rowCount !== null && (
          <p className="clear-count">
            "{table}" currently has <strong>{rowCount}</strong> row{rowCount !== 1 ? 's' : ''}.
          </p>
        )}
        <p className="clear-prompt">Type the table name to confirm:</p>
        <input
          className={`insert-input${confirmInput && confirmInput !== table ? ' input-error' : ''}`}
          value={confirmInput}
          placeholder={table}
          onChange={(e) => setConfirmInput(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter' && canSubmit) submit() }}
          autoFocus
        />
      </ModalBody>
      <ModalFooter>
        {footerError && <span className="modal-footer-error">{footerError}</span>}
        <button className="btn" onClick={onClose}>Cancel</button>
        <button
          className="btn btn-danger"
          disabled={!canSubmit}
          onClick={submit}
        >
          {submitting ? 'Clearing…' : 'Clear'}
        </button>
      </ModalFooter>
    </Modal>
  )
}
