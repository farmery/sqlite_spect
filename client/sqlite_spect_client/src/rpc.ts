// Framework-agnostic JSON-RPC 2.0 client over a WebSocket.
// - `call(method, params)` → Promise<result> (correlated by id)
// - `on(method, handler)` → subscribe to server-initiated notifications
// - `onState(handler)` → observe connection lifecycle

export type ConnState = 'connecting' | 'open' | 'closed' | 'error'

export type RpcError = { code: number; message: string }

type Pending = { resolve: (v: unknown) => void; reject: (e: RpcError) => void }
type NotificationHandler = (params: unknown) => void
type StateHandler = (s: ConnState) => void

export class RpcClient {
  private ws: WebSocket
  private nextId = 1
  private pending = new Map<number, Pending>()
  private handlers = new Map<string, Set<NotificationHandler>>()
  private stateHandlers = new Set<StateHandler>()
  private _state: ConnState = 'connecting'

  constructor(url: string) {
    this.ws = new WebSocket(url)
    this.ws.onopen = () => this.setState('open')
    this.ws.onclose = () => this.setState('closed')
    this.ws.onerror = () => this.setState('error')
    this.ws.onmessage = (ev) => this.handleMessage(ev.data as string)
  }

  get state(): ConnState {
    return this._state
  }

  call<T = unknown>(method: string, params?: unknown): Promise<T> {
    return new Promise((resolve, reject) => {
      if (this.ws.readyState !== WebSocket.OPEN) {
        reject({ code: -1, message: 'not connected' })
        return
      }
      const id = this.nextId++
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
      this.ws.send(JSON.stringify({ jsonrpc: '2.0', method, params, id }))
    })
  }

  on(method: string, handler: NotificationHandler): () => void {
    let set = this.handlers.get(method)
    if (!set) {
      set = new Set()
      this.handlers.set(method, set)
    }
    set.add(handler)
    return () => set!.delete(handler)
  }

  onState(handler: StateHandler): () => void {
    this.stateHandlers.add(handler)
    handler(this._state)
    return () => this.stateHandlers.delete(handler)
  }

  close() {
    this.ws.close()
  }

  private setState(s: ConnState) {
    this._state = s
    this.stateHandlers.forEach((h) => h(s))
  }

  private handleMessage(text: string) {
    let msg: {
      id?: number | null
      result?: unknown
      error?: RpcError
      method?: string
      params?: unknown
    }
    try {
      msg = JSON.parse(text)
    } catch {
      return
    }

    if (msg.id != null && this.pending.has(msg.id)) {
      const p = this.pending.get(msg.id)!
      this.pending.delete(msg.id)
      if (msg.error) p.reject(msg.error)
      else p.resolve(msg.result)
      return
    }

    if (msg.method) {
      this.handlers.get(msg.method)?.forEach((h) => h(msg.params))
    }
  }
}
