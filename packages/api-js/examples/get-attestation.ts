import { createInterface } from 'node:readline/promises'
import { Pythia } from '../src/index.js'

const pythia = new Pythia()

const rl = createInterface({ input: process.stdin, output: process.stdout })

const time = await rl.question('When? ')

const result = await pythia.getAttestation({
  assetPair: 'btc_usd',
  time: new Date(time),
})

console.log(JSON.stringify(result, null, 2))
