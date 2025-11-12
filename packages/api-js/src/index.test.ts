import {
  assert,
  afterEach,
  beforeEach,
  describe,
  expect,
  test,
  vi,
} from 'vitest'
import { Pythia } from './index.js'
import type {
  PythiaAnnouncement,
  PythiaAttestation,
  PythiaEvent,
  PythiaSubscriptionAnnouncement,
  PythiaSubscriptionAttestation,
} from './messages.js'

describe('Pythia', () => {
  let pythia: Pythia

  beforeEach(() => {
    pythia = new Pythia({ url: 'http://localhost:8000' })
  })

  afterEach(() => {
    pythia.disconnect()
  })

  describe('constructor', () => {
    test('should create instance with default values', () => {
      const pythia = new Pythia()
      expect(pythia.version).toBe('v1')
      expect(pythia.url).toBe('http://localhost:8000')
    })

    test('should create instance with custom values', () => {
      const pythia = new Pythia({
        version: 'v2',
        url: 'https://api.example.com',
      })
      expect(pythia.version).toBe('v2')
      expect(pythia.url).toBe('https://api.example.com')
    })

    test('should use environment variable for URL', () => {
      const originalEnv = process.env.PYTHIA_URL
      process.env.PYTHIA_URL = 'https://env.example.com'

      const pythia = new Pythia()
      expect(pythia.url).toBe('https://env.example.com')

      process.env.PYTHIA_URL = originalEnv
    })
  })

  describe('connect', () => {
    test('should connect to WebSocket successfully', async () => {
      await pythia.connect()
      let called = false
      pythia.websocket?.on('message', () => {
        called = true
      })
      // Check that we receive something
      await vi.waitFor(() => {
        if (!called) {
          throw new Error('Message not received')
        }
      })
      expect(called).toBe(true)

      let disconnected = false
      pythia.websocket?.on('close', () => {
        disconnected = true
      })
      pythia.disconnect()
      await vi.waitFor(() => {
        if (!disconnected) {
          throw new Error('Disconnected not received')
        }
      })
      expect(disconnected).toBe(true)
    })
  })

  describe('WebSocket message handling', () => {
    beforeEach(async () => {
      await pythia.connect()
    })

    test('should emit announcement event', async () => {
      const notReceivingAnnouncement = new Promise((resolve, reject) => {
        setTimeout(resolve, 1000)
        pythia.on('btc_usd/announcement', reject)
      })

      await expect(notReceivingAnnouncement).resolves.toBeUndefined()

      let announcementSpy:
        | PythiaEvent<PythiaSubscriptionAnnouncement, PythiaAnnouncement>
        | undefined
      pythia.on('btc_usd/announcement', (announcement) => {
        announcementSpy = announcement
      })
      pythia.subscribe({ assetPair: 'btc_usd', type: 'announcement' })

      await vi.waitFor(
        () => {
          if (!announcementSpy) {
            throw new Error('Announcement not received')
          }
        },
        { timeout: 2000 }
      )

      assert(announcementSpy)

      expect(announcementSpy.channel).toBe('btc_usd/announcement')
      expect(
        announcementSpy.data.oracleEvent.eventDescriptor.digitDecompositionEvent
      ).toStrictEqual({
        base: 2,
        isSigned: false,
        unit: 'usd/btc',
        precision: 0,
        nbDigits: 20,
      })
    })

    test('should emit attestation event', async () => {
      let attestationSpy:
        | PythiaEvent<PythiaSubscriptionAttestation, PythiaAttestation>
        | undefined
      pythia.on('btc_usd/attestation', (attestation) => {
        attestationSpy = attestation
      })

      await vi.waitFor(() => {
        if (!attestationSpy) {
          throw new Error('Attestation not received')
        }
      })
      assert(attestationSpy)
      expect(attestationSpy.channel).toBe('btc_usd/attestation')
      expect(attestationSpy.data.values.length).toBe(20)
      expect(attestationSpy.data.signatures.length).toBe(20)

      pythia.unsubscribe({ assetPair: 'btc_usd', type: 'attestation' })

      const notReceivingAttestation = new Promise((resolve, reject) => {
        setTimeout(resolve, 1000)
        pythia.on('btc_usd/attestation', reject)
      })

      await expect(notReceivingAttestation).resolves.toBeUndefined()
    })
  })
})
