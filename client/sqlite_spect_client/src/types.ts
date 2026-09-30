export type DbInfo = {
  id: string
  path: string
  capabilities: string[]
}

export type SchemaObject = { name: string; sql: string | null }
export type Schema = {
  tables: SchemaObject[]
  indexes: SchemaObject[]
  triggers: SchemaObject[]
  views: SchemaObject[]
}

export type Cell = string | number | boolean | null
export type Row = Cell[]

export type QueryResult = {
  columns: string[]
  rows: Row[]
  hasMore: boolean
  durationMicros: number
}

export type RowKey =
  | { kind: 'pk'; cols: string[] }
  | { kind: 'rowid'; alias: string }
  | { kind: 'none' }

export type TableRowsResult = {
  columns: string[]
  rows: Row[]
  hasMore: boolean
  durationMicros: number
  rowKey: RowKey
}

export type TableColumn = {
  cid: number
  name: string
  type: string
  notNull: boolean
  default: string | null
  pk: number
}

export type TableIndex = {
  name: string
  columns: { seqno: number; name: string }[]
  unique: boolean
}

export type ForeignKey = {
  id: number
  table: string
  from: string
  to: string | null
  onUpdate: string
  onDelete: string
}

export type TableInfo = {
  columns: TableColumn[]
  indexes: TableIndex[]
  foreignKeys: ForeignKey[]
}

export type ModalRequest =
  | { kind: 'schema'; db: string; table: string }
  | { kind: 'insert'; db: string; table: string }
  | { kind: 'clear'; db: string; table: string }
  | null

export type ExecuteResult = {
  rowsAffected: number
  lastInsertRowid?: number
  durationMicros: number
}

export type Selection =
  | { kind: 'table'; db: string; table: string }
  | { kind: 'sql'; db: string }

export type MutationEvent = { db: string; dataVersion: number }
