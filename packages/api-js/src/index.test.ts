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
import { type Expiry, asEventId, assertExpiry, unpackEventId } from './types.js'

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

      const quotingExpiry = getQuotingExpiry()
      let announcementSpyExpiry:
        | PythiaEvent<PythiaSubscriptionAnnouncement, PythiaAnnouncement>
        | undefined
      pythia.on(`btc_usd/${quotingExpiry}/announcement`, (announcement) => {
        announcementSpyExpiry = announcement
      })
      pythia.subscribe({
        assetPair: 'btc_usd',
        type: 'announcement',
        expiry: quotingExpiry,
      })

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

      assert(announcementSpy.data.oracleEvent.eventId)

      const eventId = announcementSpy.data.oracleEvent.eventId

      await vi.waitFor(
        () => {
          if (!announcementSpyExpiry) {
            throw new Error('Announcement not received')
          }
        },
        { timeout: 2000 }
      )

      assert(announcementSpyExpiry)

      expect(announcementSpyExpiry.channel).toBe(
        `btc_usd/${quotingExpiry}/announcement`
      )
      expect(
        announcementSpyExpiry.data.oracleEvent.eventDescriptor
          .digitDecompositionEvent
      ).toStrictEqual({
        base: 2,
        isSigned: false,
        unit: 'usd/btc',
        precision: 0,
        nbDigits: 20,
      })

      assert(announcementSpy.data.oracleEvent.eventId)

      const eventIdExpiry = announcementSpyExpiry.data.oracleEvent.eventId

      const { assetPair, time } = unpackEventId(eventIdExpiry)

      expect(asEventId(assetPair, time)).toBe(eventId)
    })

    test('should emit attestation event', async () => {
      let attestationSpy:
        | PythiaEvent<PythiaSubscriptionAttestation, PythiaAttestation>
        | undefined
      pythia.on('btc_usd/attestation', (attestation) => {
        attestationSpy = attestation
      })

      const quotingExpiry = getQuotingExpiry()
      let attestationSpyExpiry:
        | PythiaEvent<PythiaSubscriptionAttestation, PythiaAttestation>
        | undefined
      pythia.on(`btc_usd/${quotingExpiry}/attestation`, (attestation) => {
        attestationSpyExpiry = attestation
      })
      pythia.subscribe({
        assetPair: 'btc_usd',
        type: 'attestation',
        expiry: quotingExpiry,
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

      assert(attestationSpy.data.eventId)

      const eventId = attestationSpy.data.eventId

      await vi.waitFor(
        () => {
          if (!attestationSpyExpiry) {
            throw new Error('Attestation not received')
          }
        },
        { timeout: 2000 }
      )

      assert(attestationSpyExpiry)

      expect(attestationSpyExpiry.channel).toBe(
        `btc_usd/${quotingExpiry}/attestation`
      )
      expect(attestationSpyExpiry.data.values.length).toBe(20)
      expect(attestationSpyExpiry.data.signatures.length).toBe(20)

      assert(attestationSpy.data.eventId)

      const eventIdExpiry = attestationSpyExpiry.data.eventId

      const { assetPair, time } = unpackEventId(eventIdExpiry)

      expect(asEventId(assetPair, time)).toBe(eventId)

      pythia.unsubscribe({ assetPair: 'btc_usd', type: 'attestation' })

      const notReceivingAttestation = new Promise((resolve, reject) => {
        setTimeout(resolve, 1000)
        pythia.on('btc_usd/attestation', reject)
      })

      await expect(notReceivingAttestation).resolves.toBeUndefined()
    })

    test('should emit all expiries events', async () => {
      const expiriesAnnouncement = new Set<Expiry>()
      pythia.on('btc_usd/ALL/announcement' as const, (announcement) => {
        const { expiry } = unpackEventId(announcement.data.oracleEvent.eventId)
        assert(expiry)
        expiriesAnnouncement.add(expiry)
      })
      pythia.subscribe({
        assetPair: 'btc_usd',
        type: 'announcement',
        expiry: 'ALL',
      })

      const expiriesAttestation = new Set<Expiry>()
      pythia.on('btc_usd/ALL/attestation' as const, (attestation) => {
        const { expiry } = unpackEventId(attestation.data.eventId)
        assert(expiry)
        expiriesAttestation.add(expiry)
      })
      pythia.subscribe({
        assetPair: 'btc_usd',
        type: 'attestation',
        expiry: 'ALL',
      })

      await vi.waitFor(
        () => {
          if (!expiriesAnnouncement.size) {
            throw new Error('Announcement not received')
          }
        },
        { timeout: 2000 }
      )

      await vi.waitFor(
        () => {
          if (!expiriesAttestation.size) {
            throw new Error('Attestation not received')
          }
        },
        { timeout: 2000 }
      )

      expect(expiriesAnnouncement.size).toBeGreaterThan(2)
      expect(expiriesAnnouncement.size).toBeOneOf([
        expiriesAttestation.size - 1,
        expiriesAttestation.size,
        expiriesAttestation.size + 1,
      ])

      const quotingExpiry = getQuotingExpiry()

      expect(expiriesAttestation).toContain(quotingExpiry)

      const intersection = new Set(
        [...expiriesAttestation].filter((x) => expiriesAnnouncement.has(x))
      )

      expect(intersection.size).toBeGreaterThan(expiriesAttestation.size - 1)
    })
  })
})

const getQuotingExpiry: () => Expiry = () => {
  const now = new Date(Date.now())
  const year = now.getFullYear().toString().slice(-2)
  const month = now.toString().slice(4, 7).toUpperCase()
  const day = now.getDate()

  const laterDay = day > 28 ? 1 : day + 1
  let expiry = `${laterDay}${month}${year}`

  if (expiry.length !== 7) {
    expiry = `0${expiry}`
  }

  return assertExpiry(expiry)
}
