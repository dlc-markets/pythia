import { Pythia } from '../src/index.js'
import { createInterface } from 'node:readline/promises'

const pythia = new Pythia()

const rl = createInterface({ input: process.stdin, output: process.stdout })

const time = await rl.question('When? ')
const price = await rl.question('At which price? ')

const result = await pythia.forceAttestation({
  time: new Date(time),
  price: Number(price),
})

console.log(JSON.stringify(result, null, 2))
