import { Pythia } from '../src/index.js'

const pythia = new Pythia()

const result = await pythia.getOraclePublicKey()

console.log(result)
