import { Pythia } from '../src/index.js'
import { createInterface } from 'node:readline/promises'

const pythia = new Pythia()

const rl = createInterface({ input: process.stdin, output: process.stdout })

const assetPair = await rl.question('Which assetPair? ')

const result = await pythia.getAsset({ assetPair })

console.log(result)
