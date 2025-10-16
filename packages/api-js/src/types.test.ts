import { assertEventId, assertExpiry, unpackEventId } from './types.js'

import { describe, expect, test } from 'vitest'

describe('parseEventId', () => {
  test('should parse spot event id correctly', () => {
    const result = unpackEventId(assertEventId('btc_usd1706272800'))

    expect(result.assetPair).toBe('btc_usd')
    expect(result.time).toStrictEqual(new Date(1706272800 * 1000))
    expect(result.expiry).toBeUndefined()
  })

  test('should parse forward event id correctly', () => {
    const result = unpackEventId(assertEventId('btc_usd_25DEC24f1706272800'))

    expect(result.assetPair).toBe('btc_usd')
    expect(result.expiry).toBe('25DEC24')
    expect(result.time).toStrictEqual(new Date(1706272800 * 1000))
  })

  test('should parse delivery event id correctly', () => {
    const result = unpackEventId(assertEventId('btc_usd_25DEC24d'))

    expect(result.assetPair).toBe('btc_usd')
    expect(result.expiry).toBe('25DEC24')
    // Delivery time should be the expiry date at 8 AM UTC
    expect(result.time).toStrictEqual(new Date(2024, 11, 25, 8, 0, 0, 0)) // December 25, 2024 8:00 AM UTC
  })

  test('should throw error for invalid event id format', () => {
    expect(() => assertEventId('invalid_format')).toThrow(
      'Invalid EventId format: invalid_format. Expected one of:\n  - Spot: {asset_pair}{unix_timestamp} (e.g., "btc_usd1706272800")\n  - Forward: {asset_pair}_{expiry}f{unix_timestamp} (e.g., "btc_usd_25DEC24f1706272800")\n  - Delivery: {asset_pair}_{expiry}d (e.g., "btc_usd_25DEC24d")'
    )
  })
})

describe('assertExpiry', () => {
  test('should accept valid expiry formats', () => {
    expect(assertExpiry('25DEC24')).toBe('25DEC24')
    expect(assertExpiry('01JAN25')).toBe('01JAN25')
    expect(assertExpiry('31MAR23')).toBe('31MAR23')
    expect(assertExpiry('15aug26')).toBe('15AUG26') // Should convert to uppercase
  })

  test('should reject invalid day values', () => {
    expect(() => assertExpiry('00DEC24')).toThrow(
      'Invalid day in expiry: 0. Day must be between 1 and 31'
    )
    expect(() => assertExpiry('32JAN25')).toThrow(
      'Invalid day in expiry: 32. Day must be between 1 and 31'
    )
  })

  test('should reject invalid month values', () => {
    expect(() => assertExpiry('31XYZ23')).toThrow(
      'Invalid expiry format: 31XYZ23. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
    expect(() => assertExpiry('05ABC95')).toThrow(
      'Invalid expiry format: 05ABC95. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
  })

  test('should reject invalid year values', () => {
    expect(() => assertExpiry('25DEC-1')).toThrow(
      'Invalid expiry format: 25DEC-1. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
    expect(() => assertExpiry('25DEC100')).toThrow(
      'Invalid expiry format: 25DEC100. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
  })

  test('should reject invalid formats', () => {
    expect(() => assertExpiry('25DEC')).toThrow(
      'Invalid expiry format: 25DEC. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
    expect(() => assertExpiry('DEC24')).toThrow(
      'Invalid expiry format: DEC24. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
    expect(() => assertExpiry('25DEC2024')).toThrow(
      'Invalid expiry format: 25DEC2024. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
    expect(() => assertExpiry('25-12-24')).toThrow(
      'Invalid expiry format: 25-12-24. Date must be a Deribit expiry like "25DEC24" or "01JAN25"'
    )
  })
})

describe('assertEventId', () => {
  test('should accept valid spot EventIds', () => {
    expect(assertEventId('btc_usd1706272800')).toBe('btc_usd1706272800')
    expect(() => assertEventId('eth_usd1706272800')).toThrow(
      'Invalid asset pair: eth_usd. Currently only "btc_usd" is supported'
    )
  })

  test('should accept valid forward EventIds', () => {
    expect(assertEventId('btc_usd_25DEC24f1706272800')).toBe(
      'btc_usd_25DEC24f1706272800'
    )
  })

  test('should accept valid delivery EventIds', () => {
    expect(assertEventId('btc_usd_25DEC24d')).toBe('btc_usd_25DEC24d')
  })

  test('should reject EventIds with invalid expiry', () => {
    expect(() => assertEventId('btc_usd_32DEC24f1706272800')).toThrow(
      'Invalid day in expiry: 32. Day must be between 1 and 31'
    )
    expect(() => assertEventId('btc_usd_00DEC24d')).toThrow(
      'Invalid day in expiry: 0. Day must be between 1 and 31'
    )
  })

  test('should reject EventIds with unreasonable timestamps', () => {
    // Too far in the past
    expect(() => assertEventId('btc_usd1000000000')).toThrow(
      'Invalid timestamp in EventId: 1000000000. Timestamp seems unreasonable'
    )
    // Too far in the future
    expect(() => assertEventId('btc_usd9999999999')).toThrow(
      'Invalid timestamp in EventId: 9999999999. Timestamp seems unreasonable'
    )
  })

  test('should reject invalid EventId formats', () => {
    expect(() => assertEventId('invalid_format')).toThrow(
      'Invalid EventId format: invalid_format'
    )
    expect(() => assertEventId('btc_usd_25DEC24')).toThrow(
      'Invalid EventId format: btc_usd_25DEC24'
    )
    expect(() => assertEventId('btc_usd_25DEC24x1706272800')).toThrow(
      'Invalid EventId format: btc_usd_25DEC24x1706272800'
    )
  })
})
