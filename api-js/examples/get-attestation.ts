import { Pythia } from '../src/index.js'
import { isRFC3339DateTime } from './utils.js'
import readline from 'node:readline'

const pythia = new Pythia()

try {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  })
  const time = (await new Promise((resolve, reject) => {
    rl.question('When?', (time) => {
      if (!time || !isRFC3339DateTime(time)) {
        reject('Invalid time, must be RFC3339 format')
      }
      resolve(time)
      rl.close()
    })
  })) as string
  const result = await pythia.getAttestation({ pair: 'btcusd', time })
  console.log(result)
} catch (e) {
  console.error(e)
}
