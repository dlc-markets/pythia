import process from 'node:process'

export interface PublicKey {
  public_key: string
}

export class Pythia {
  url: string
  version: string

  constructor() {
    this.version = 'v1'
    this.url = process.env.PYTHIA_URL || 'http://localhost:8000'
  }

  async request<Result>(
    method: string,
    path: string,
    params?: unknown
  ): Promise<Result> {
    const url = new URL(`${this.version}/${path}`, this.url)
    let body = undefined
    if (method === 'GET' && params) {
      for (const [key, value] of Object.entries(params)) {
        url.searchParams.append(key, value as string)
      }
    } else if (method === 'POST' && params) {
      body = JSON.stringify(params)
    }
    const headers = { 'Content-Type': 'application/json' }
    const options = { method, headers, body }
    const response = await fetch(url.href, options)
    if (!response.ok) throw new Error(response.statusText)
    const json: Result = (await response.json()) as Result
    return json
  }

  getOraclePublicKey() {
    return this.request<PublicKey>('GET', 'oracle/publickey')
  }

  getAssets() {
    return this.request('GET', 'asset')
  }

  getAsset(pair: string) {
    return this.request('GET', `asset/${pair}/config`)
  }

  getAnnouncement({ pair, time }: { pair: string; time: string }) {
    return this.request('GET', `asset/${pair}/announcement/${time}`)
  }

  getAttestation({ pair, time }: { pair: string; time: string }) {
    return this.request('GET', `asset/${pair}/attestation/${time}`)
  }

  forceAttestation({ time, price }: { time: string; price: number }) {
    return this.request('POST', 'force', { maturation: time, price })
  }
}
