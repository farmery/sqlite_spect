import { useRpc } from '../RpcContext'

export function EmptyState() {
  const { state } = useRpc()
  if (state === 'connecting')
    return (
      <div className="empty-state">
        <p>Connecting…</p>
      </div>
    )
  if (state === 'error' || state === 'closed')
    return (
      <div className="empty-state">
        <h2>Not connected</h2>
        <p>Check that the inspector CLI is running and reachable.</p>
      </div>
    )
  return (
    <div className="empty-state">
      <h2>Select a table</h2>
      <p>Pick a database on the left, then choose a table to browse rows or open the SQL runner.</p>
    </div>
  )
}
