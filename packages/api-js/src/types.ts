type Month =
  | 'JAN'
  | 'FEB'
  | 'MAR'
  | 'APR'
  | 'MAY'
  | 'JUN'
  | 'JUL'
  | 'AUG'
  | 'SEP'
  | 'OCT'
  | 'NOV'
  | 'DEC'

export type AssetPair = 'btc_usd'
export type Expiry = `${number}${Month}${number}`
export type EventId =
  `${AssetPair}${`_${Expiry}${`f${number}` | 'd'}` | `${number}`}`

/**
 * Asserts that a string is a valid AssetPair and returns it as AssetPair type
 * @param value - The string to validate
 * @returns The string as AssetPair type if valid
 * @throws Error if the string is not a valid AssetPair format
 */
export const assertAssetPair = (value: string): AssetPair => {
  // Currently only btc_usd is supported
  if (value !== 'btc_usd') {
    throw new Error(
      `Invalid asset pair: ${value}. Currently only "btc_usd" is supported`
    )
  }

  return 'btc_usd' as AssetPair
}

/**
 * Asserts that a string is a valid Expiry and returns it as Expiry type
 * @param value - The string to validate
 * @returns The string as Expiry type if valid
 * @throws Error if the string is not a valid Expiry format
 */
export const assertExpiry = (value: string): Expiry => {
  // Validate format: (e.g., "25DEC24")
  const expiryRegex =
    /^([0-9]{1,2})(JAN|FEB|MAR|APR|MAY|JUN|JUL|AUG|SEP|OCT|NOV|DEC)([0-9]{2})$/i
  const match = value.match(expiryRegex)

  if (!match || !match[1] || !match[2] || !match[3]) {
    throw new Error(
      `Invalid expiry format: ${value}. Date must be a Deribit expiry like "25DEC24"`
    )
  }

  const dayNum = Number.parseInt(match[1], 10)

  // Validate day range (1-31)
  if (dayNum < 1 || dayNum > 31) {
    throw new Error(
      `Invalid day in expiry: ${dayNum}. Day must be between 1 and 31`
    )
  }

  return value.toUpperCase() as Expiry
}

/**
 * Asserts that a string is a valid EventId and returns it as EventId type
 * @param value - The string to validate
 * @returns The string as EventId type if valid
 * @throws Error if the string is not a valid EventId format
 */
export const assertEventId = (value: string): EventId => {
  // Test for Delivery format: {asset_pair}_{expiry}d
  const deliveryRegex = /^([a-z_]+)_([0-9]{1,2}[A-Z]{3}[0-9]{2})d$/i
  if (deliveryRegex.test(value)) {
    const match = value.match(deliveryRegex)
    if (match?.[1] && match[2]) {
      const [, assetPair, expiry] = match
      // Validate the asset pair
      assertAssetPair(assetPair)
      // Validate the expiry part
      assertExpiry(expiry)
      return value as EventId
    }
  }

  // Test for Forward format: {asset_pair}_{expiry}f{unix_timestamp}
  const forwardRegex = /^([a-z_]+)_([0-9]{1,2}[A-Z]{3}[0-9]{2})f(\d+)$/i
  if (forwardRegex.test(value)) {
    const match = value.match(forwardRegex)
    if (match?.[1] && match[2] && match[3]) {
      const [, assetPair, expiry, timestamp] = match
      // Validate the asset pair
      assertAssetPair(assetPair)
      // Validate the expiry part
      assertExpiry(expiry)
      // Validate timestamp is reasonable (not too far in past/future)
      const timestampNum = Number.parseInt(timestamp, 10)
      const now = Math.floor(Date.now() / 1000)
      if (
        timestampNum < now - 31536000 * 10 ||
        timestampNum > now + 31536000 * 10
      ) {
        // ±10 years
        throw new Error(
          `Invalid timestamp in EventId: ${timestamp}. Timestamp seems unreasonable`
        )
      }
      return value as EventId
    }
  }

  // Test for Spot format: {asset_pair}{unix_timestamp}
  const spotRegex = /^([a-z_]+)(\d+)$/i
  if (spotRegex.test(value)) {
    const match = value.match(spotRegex)
    if (match?.[1] && match[2]) {
      const [, assetPair, timestamp] = match
      // Validate the asset pair
      assertAssetPair(assetPair)
      // Validate timestamp is reasonable (not too far in past/future)
      const timestampNum = Number.parseInt(timestamp, 10)
      const now = Math.floor(Date.now() / 1000)
      if (
        timestampNum < now - 31536000 * 10 ||
        timestampNum > now + 31536000 * 10
      ) {
        // ±10 years
        throw new Error(
          `Invalid timestamp in EventId: ${timestamp}. Timestamp seems unreasonable`
        )
      }
      return value as EventId
    }
  }

  throw new Error(`Invalid EventId format: ${value}. Expected one of:
  - Spot: {asset_pair}{unix_timestamp} (e.g., "btc_usd1706272800")
  - Forward: {asset_pair}_{expiry}f{unix_timestamp} (e.g., "btc_usd_25DEC24f1706272800")
  - Delivery: {asset_pair}_{expiry}d (e.g., "btc_usd_25DEC24d")`)
}

const monthToNumber: Record<Month, number> = {
  JAN: 0,
  FEB: 1,
  MAR: 2,
  APR: 3,
  MAY: 4,
  JUN: 5,
  JUL: 6,
  AUG: 7,
  SEP: 8,
  OCT: 9,
  NOV: 10,
  DEC: 11,
}

export const asDateExpiry = (expiry: Expiry): Date => {
  // Extract day, month, and year from expiry string (e.g., "25DEC24")
  const day = Number.parseInt(expiry.slice(0, -5)) // Get day (first part)
  const monthStr = expiry.slice(-5, -2) as Month // Get month (middle part)
  const year = Number.parseInt(expiry.slice(-2)) // Get year (last part)

  // Convert 2-digit year to 4-digit year (assuming 20xx for years 00-99)
  const fullYear = 2000 + year

  // Create date at 8 AM
  const date = new Date(fullYear, monthToNumber[monthStr], day, 8, 0, 0, 0)

  return date
}

export const asEventId = (
  assetPair: AssetPair,
  time: Date,
  expiry?: Expiry
): EventId => {
  if (expiry) {
    const expiryAsDate = asDateExpiry(expiry)
    return expiryAsDate === time
      ? (`${assetPair}_${expiry}d` as EventId)
      : (`${assetPair}_${expiry}f${time.getTime() / 1000}` as EventId)
  }
  return `${assetPair}${time.getTime() / 1000}`
}

export const unpackEventId = (
  eventId: EventId
): { assetPair: AssetPair; expiry?: Expiry; time: Date } => {
  // EventId is supposed to be already asserted, so we can safely cast slices of it as string
  const assetPair = eventId.slice(0, 7) as AssetPair

  // We must still check if there an expiry in it
  try {
    const expiry = assertExpiry(eventId.slice(8, 15))
    const timestamp = Number.parseInt(eventId.slice(16), 10)
    return Number.isNaN(timestamp)
      ? {
          assetPair,
          expiry,
          time: asDateExpiry(expiry),
        }
      : {
          assetPair,
          expiry,
          time: new Date(timestamp * 1000),
        }
  } catch {
    const time = new Date(Number.parseInt(eventId.slice(7), 10) * 1000)
    return {
      assetPair,
      time,
    }
  }
}
