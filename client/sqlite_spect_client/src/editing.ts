// Data editing is temporarily disabled in the inspector UI while a bug in the
// mutation path is investigated. This gate is client-side only — the server
// RPCs (db.updateRow, db.insertRow, db.deleteRow, db.clearTable) are untouched,
// and the SQL Runner still executes arbitrary SQL (behind a warning).
// Flip EDITING_ENABLED back to true to restore the editing UI.
export const EDITING_ENABLED: boolean = false

export const SQL_WRITE_WIP_WARNING =
  'Edits and modifications are a work in progress. A statement that modifies your database might work — but might corrupt it. Prefer read-only (SELECT) queries for now.'
