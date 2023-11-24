import { Pythia } from '../index.js'

const pythia = new Pythia()

const main = async () => {
    const result = await pythia.getAssets()
    console.log(result)
    }
    
    main()