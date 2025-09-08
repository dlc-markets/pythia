import { createInterface } from 'node:readline/promises'
import { Pythia } from '../src/index.js'

const pythia = new Pythia()

const rl = createInterface({ input: process.stdin, output: process.stdout })

const assetPair = await rl.question('Which assetPair? ')

if (assetPair !== 'btc_usd') {
  throw new Error('Only btc_usd is supported')
}

const result = await pythia.getConfig({ assetPair: 'btc_usd' as const })

console.log(result)
