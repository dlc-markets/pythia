import { Pythia } from '../src/index.js'

const pythia = new Pythia()

await pythia.connect()

pythia.on('btcusd/attestation', (attestation) => {
  console.log('new attestation')
  console.log(attestation)
})

pythia.on('btcusd/announcement', (announcement) => {
  console.log('new announcement')
  console.log(announcement)
})
