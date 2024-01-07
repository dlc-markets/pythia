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
    eventId: string
  }
}

interface PythiaAttestation {
  eventId: string
  signatures: string[]
  values: string[]
}

interface Constructor {
  version?: string
  url?: string
}

export class Pythia {
  url: string
  version: string

  constructor(options: Constructor = {}) {
    this.version = options.version || 'v1'
    this.url = options.url || 'http://localhost:8000'
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
    return this.request<string[]>('GET', 'asset')
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

  getAttestation({ assetPair, time }: { assetPair: string; time: Date }) {
    return this.request<PythiaAttestation>(
      'GET',
      `asset/${assetPair}/attestation/${time.toISOString()}`
    )
  }

  getAttestationByEventId({ eventId }: { eventId: string }) {
    const match = eventId.match(/([a-z]+)(\d+)/i)?.slice(1)
    if (!match || match.length !== 2) {
      throw new Error('Invalid event id')
    }
    const [assetPair, time] = match as [string, string]
    return this.getAttestation({ assetPair, time: new Date(+time * 1000) })
  }

  forceAttestation({ time, price }: { time: Date; price: number }) {
    return this.request<{
      announcement: PythiaAnnouncement
      attestation: PythiaAttestation
    }>('POST', 'force', { maturation: time.toISOString(), price })
  }
}
