import { useState } from 'react'
import { RpcProvider } from './RpcContext'
import { EmptyState } from './components/EmptyState'
import { Sidebar } from './components/Sidebar'
import { SqlRunner } from './components/SqlRunner'
import { TableView } from './components/TableView'
import type { ModalRequest, Selection } from './types'
import './App.css'

// Modals — loaded lazily via dynamic import would be ideal, but for simplicity
// we import them directly; they render nothing when `modal === null`.
import { SchemaModal } from './components/modals/SchemaModal'
import { InsertModal } from './components/modals/InsertModal'
import { ClearConfirmModal } from './components/modals/ClearConfirmModal'

function App() {
  const [selection, setSelection] = useState<Selection | null>(null)
  const [modal, setModal] = useState<ModalRequest>(null)

  return (
    <RpcProvider>
      <div className="app">
        <Sidebar
          selection={selection}
          onSelect={setSelection}
          onOpenModal={setModal}
        />
        <main className="main">
          {selection === null && <EmptyState />}
          {selection?.kind === 'table' && (
            <TableView
              key={`${selection.db}/${selection.table}`}
              db={selection.db}
              table={selection.table}
              onOpenModal={setModal}
            />
          )}
          {selection?.kind === 'sql' && <SqlRunner key={selection.db} db={selection.db} />}
        </main>
      </div>

      {modal?.kind === 'schema' && (
        <SchemaModal
          db={modal.db}
          table={modal.table}
          onClose={() => setModal(null)}
        />
      )}
      {modal?.kind === 'insert' && (
        <InsertModal
          db={modal.db}
          table={modal.table}
          onClose={() => setModal(null)}
          onSuccess={() => setModal(null)}
        />
      )}
      {modal?.kind === 'clear' && (
        <ClearConfirmModal
          db={modal.db}
          table={modal.table}
          onClose={() => setModal(null)}
          onSuccess={() => setModal(null)}
        />
      )}
    </RpcProvider>
  )
}

export default App
