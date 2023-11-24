import { Pythia } from '../src/index.js'
import readline from 'node:readline'

const pythia = new Pythia()

try {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  })
  const asset: string = await new Promise((resolve, reject) => {
    rl.question('Enter asset: ', (asset) => {
      if (!asset) {
        reject('Invalid asset')
      }
      resolve(asset)
      rl.close()
    })
  })
  const result = await pythia.getAsset(asset)
  console.log(result)
} catch (e) {
  console.error(e)
}
