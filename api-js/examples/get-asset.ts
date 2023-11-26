import { Pythia } from '../src/index.js'
import { createInterface } from 'node:readline/promises'

const pythia = new Pythia()

const rl = createInterface({ input: process.stdin, output: process.stdout })

const asset = await rl.question('Which asset? ')

const result = await pythia.getAsset({ asset })

console.log(result)
