import { Pythia } from '../index.js'

const pythia = new Pythia()

const main = async () => {
    const result = await pythia.getOraclePublicKey()
    console.log(result)
    }
    
    void main()