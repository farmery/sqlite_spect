import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react'

export function useColResize(colCount: number) {
  const [colWidths, setColWidths] = useState<number[]>([])
  const resizingRef = useRef<{ colIdx: number; startX: number; startWidth: number } | null>(null)

  // Initialise widths once columns are known; gated so user resizes aren't overwritten.
  useEffect(() => {
    if (colCount > 0 && colWidths.length === 0) {
      setColWidths(Array(colCount).fill(160))
    }
  })

  function onResizeStart(e: ReactMouseEvent, colIdx: number) {
    e.preventDefault()
    resizingRef.current = { colIdx, startX: e.clientX, startWidth: colWidths[colIdx] ?? 160 }

    function onMove(ev: MouseEvent) {
      if (!resizingRef.current) return
      const { colIdx: ci, startX, startWidth } = resizingRef.current
      const newWidth = Math.max(60, startWidth + (ev.clientX - startX))
      setColWidths((prev) => {
        const next = [...prev]
        next[ci] = newWidth
        return next
      })
    }

    function onUp() {
      resizingRef.current = null
      document.removeEventListener('mousemove', onMove)
      document.removeEventListener('mouseup', onUp)
    }

    document.addEventListener('mousemove', onMove)
    document.addEventListener('mouseup', onUp)
  }

  return { colWidths, onResizeStart }
}
