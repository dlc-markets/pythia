import process from 'node:process'
import { EventEmitter } from 'eventemitter3'
import { WebSocket } from 'ws'

interface FetchOptions {
  method: string
  headers: Record<string, string>
  body?: string
}

interface PythiaAsset {
  pricefeed: string
  announcement_offset: string
  frequency: string
}

interface PythiaAnnouncement {
  announcementSignature: string
  oraclePublicKey: string
  oracleEvent: {
    eventId: string
    oracleNonces: string[]
    eventMaturityEpoch: number
    eventDescriptor: {
      digitDecompositionEvent: {
        base: number
        isSigned: boolean
        unit: string
        precision: number
        nbDigits: number
      }
    }
  }
}

interface PythiaAttestation {
  eventId: string
  signatures: string[]
  values: string[]
}

interface Events {
  connected: () => void
  disconnected: () => void
  [key: `${string}/attestation`]: (attestation: PythiaAttestation) => void
  [key: `${string}/announcement`]: (attestation: PythiaAnnouncement) => void
}

interface Constructor {
  version?: string
  url?: string
}

export const parseEventId = (eventId: string) => {
  const match = eventId.match(/([a-z_]+)(\d+)/i)?.slice(1)
  if (!match || match.length !== 2) {
    throw new Error('Invalid event id')
  }
  const [assetPair, time] = match as [string, string]
  return { assetPair, time: new Date(+time * 1000) }
}

export class Pythia extends EventEmitter<Events> {
  version: string
  url: string
  websocket?: WebSocket

  constructor(options: Constructor = {}) {
    super()
    this.version = options.version || 'v1'
    this.url = options.url || process.env.PYTHIA_URL || 'http://localhost:8000'
  }

  async connect() {
    this.disconnect()

    const wsUrl = `${this.url.replace('http', 'ws')}/${this.version}/ws`
    this.websocket = new WebSocket(wsUrl)

    await new Promise<void>((resolve, reject) => {
      if (!this.websocket) {
        throw new Error('WebSocket is not created')
      }

      this.websocket.on('open', () => {
        this.emit('connected')
        resolve()
      })

      this.websocket.onerror = (error) => {
        reject(error)
      }
    })

    this.websocket.on('message', (message: string) => {
      try {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const data = JSON.parse(message) as { method: string; params: any }

        if (data.method === 'subscriptions') {
          // eslint-disable-next-line @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-unsafe-argument
          this.emit(data.params.channel, data.params.data)
        }
      } catch (error) {
        // eslint-disable-next-line no-console
        console.error(error)
      }
    })

    this.websocket.on('close', () => {
      this.emit('disconnected')

      setTimeout(() => {
        void this.connect()
      }, 5000)
    })
  }

  disconnect() {
    this.websocket?.close()
  }

  async request<Result>(
    method: string,
    path: string,
    params?: unknown
  ): Promise<Result> {
    const url = new URL(`${this.version}/${path}`, this.url)

    const options = {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: undefined,
    } as FetchOptions

    if (method === 'GET' && params && Object.keys(params).length > 0) {
      for (const [key, value] of Object.entries(params)) {
        url.searchParams.append(key, value as string)
      }
    } else if (method === 'POST' && params) {
      options.body = JSON.stringify(params)
    }

    const response = await fetch(url, options)

    if (!response.ok) {
      throw new Error(response.statusText)
    }

    return response.json() as Result
  }

  getOraclePublicKey() {
    return this.request<{ publicKey: string }>('GET', 'oracle/publickey')
  }

  getAssets() {
    return this.request<string[]>('GET', 'assets')
  }

  getAsset({ assetPair }: { assetPair: string }) {
    return this.request<PythiaAsset>('GET', `asset/${assetPair}/config`)
  }

  getAnnouncement({ assetPair, time }: { assetPair: string; time: Date }) {
    return this.request<PythiaAnnouncement>(
      'GET',
      `asset/${assetPair}/announcement/${time.toISOString()}`
    )
  }

  getAnnouncements({ assetPair, times }: { assetPair: string; times: Date[] }) {
    return this.request<PythiaAnnouncement[]>(
      'POST',
      `asset/${assetPair}/announcements`,
      { times: times.map((date) => date.toISOString()) }
    )
  }

  getAnnouncementByEventId({ eventId }: { eventId: string }) {
    const { assetPair, time } = parseEventId(eventId)
    return this.getAnnouncement({ assetPair, time })
  }

  getAttestation({ assetPair, time }: { assetPair: string; time: Date }) {
    return this.request<PythiaAttestation>(
      'GET',
      `asset/${assetPair}/attestation/${time.toISOString()}`
    )
  }

  getAttestationByEventId({ eventId }: { eventId: string }) {
    const { assetPair, time } = parseEventId(eventId)
    return this.getAttestation({ assetPair, time })
  }

  forceAttestation({ time, price }: { time: Date; price: number }) {
    return this.request<{
      announcement: PythiaAnnouncement
      attestation: PythiaAttestation
    }>('POST', 'force', { maturation: time.toISOString(), price })
  }
}
