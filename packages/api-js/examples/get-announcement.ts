import { Pythia } from '../src/index.js'

import { createInterface } from 'node:readline/promises'

const pythia = new Pythia()

const rl = createInterface({ input: process.stdin, output: process.stdout })

const time = await rl.question('When? ')

const result = await pythia.getAnnouncement({
  assetPair: 'btcusd',
  time: new Date(time),
})

console.log(JSON.stringify(result, null, 2))
