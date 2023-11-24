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

  async request<Result>(path: string, method: string, params?: any): Promise<Result> {
    const url = `${this.url}/${this.version}/${path}`
    const headers = { 'Content-Type': 'application/json' }
    const options = { method, headers, body: JSON.stringify(params) }
    const response = await fetch(url, options)
    const json = await response.json()
    return json
  }

  getOraclePublicKey() {
    return this.request<PublicKey>('oracle/publickey', 'GET')
  }

  getAssets() {
    return this.request('asset', 'GET')
  }

  getAsset(assetId: string) {
    return this.request(`asset/${assetId}/config`, 'GET')
  }

  getAnnouncement({ assetId, time }: { assetId: string; time: string }) {
    return this.request(`asset/${assetId}/announcement/${time}`, 'GET')
  }

  getAttestation({ assetId, time }: { assetId: string; time: string }) {
    return this.request(`asset/${assetId}/attestation/${time}`, 'GET')
  }
}
