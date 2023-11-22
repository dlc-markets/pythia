
export class Pythia {
  url: string

  constructor() {
    this.url = process.env.PYTHIA_URL || 'http://localhost:8000'
  }
}
