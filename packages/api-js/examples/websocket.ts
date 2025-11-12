import { Pythia } from '../src/index.js'

const pythia = new Pythia()

await pythia.connect()

pythia.on('btc_usd/attestation', (attestation) => {
  console.log('new attestation')
  console.log(attestation)
})

pythia.on('btc_usd/announcement', (announcement) => {
  console.log('new announcement')
  console.log(announcement)
})
