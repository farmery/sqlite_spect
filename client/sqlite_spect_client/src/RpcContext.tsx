import { createContext, useContext, useEffect, useState, type ReactNode } from 'react'
import { RpcClient, type ConnState } from './rpc'

type RpcContextValue = { client: RpcClient | null; state: ConnState }

const RpcContext = createContext<RpcContextValue>({ client: null, state: 'connecting' })

export function RpcProvider({ children }: { children: ReactNode }) {
  const [client, setClient] = useState<RpcClient | null>(null)
  const [state, setState] = useState<ConnState>('connecting')

  useEffect(() => {
    const proto = window.location.protocol === 'https:' ? 'wss' : 'ws'
    const url = `${proto}://${window.location.host}/ws`
    // TODO: append ?auth_key=... here when auth is implemented
    const c = new RpcClient(url)
    setClient(c)
    const unsub = c.onState(setState)
    return () => {
      unsub()
      c.close()
    }
  }, [])

  return <RpcContext.Provider value={{ client, state }}>{children}</RpcContext.Provider>
}

export function useRpc(): RpcContextValue {
  return useContext(RpcContext)
}
