import process from 'node:process'

interface FetchOptions {
  method: string
  headers: Record<string, string>
  body?: string
}

interface PythiaAnnoucement {
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

export class Pythia {
  url: string
  version: string

  constructor() {
    this.version = process.env.PYTHIA_API_VERSION || 'v1'
    this.url = process.env.PYTHIA_URL || 'http://localhost:8000'
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
    return this.request<{ public_key: string }>('GET', 'oracle/publickey')
  }

  getAssets() {
    return this.request<string[]>('GET', 'asset')
  }

  getAsset({ asset }: { asset: string }) {
    return this.request<{
      pricefeed: string
      announcement_offset: string
      frequency: string
    }>('GET', `asset/${asset}/config`)
  }

  getAnnouncement({ pair, time }: { pair: string; time: Date }) {
    return this.request<PythiaAnnoucement>(
      'GET',
      `asset/${pair}/announcement/${time.toISOString()}`
    )
  }

  getAttestation({ pair, time }: { pair: string; time: Date }) {
    return this.request<PythiaAttestation>(
      'GET',
      `asset/${pair}/attestation/${time.toISOString()}`
    )
  }

  forceAttestation({ time, price }: { time: Date; price: number }) {
    return this.request<{
      annoucement: PythiaAnnoucement
      attestation: PythiaAttestation
    }>('POST', 'force', { maturation: time.toISOString(), price })
  }
}
