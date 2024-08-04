import { parseEventId } from './index.js'

import { describe, expect, test } from 'vitest'

describe('parseEventId', () => {
  test('should return the event id and time as date', () => {
    const { assetPair, time } = parseEventId('btc_usd1706272800')

    expect(assetPair).toBe('btc_usd')
    expect(time).toStrictEqual(new Date(1706272800 * 1000))
  })
})
