import { Pythia } from '../src/index.js'

const pythia = new Pythia()

await pythia.connect()

pythia.on('btcusd/attestation', (attestation) => {
  console.log(attestation)
})
