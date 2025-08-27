import type { AssetPair, EventId, Expiry } from './types.js'

export interface PythiaAsset {
  pricefeed: string
  announcement_offset: string
  frequency: string
}

export interface PythiaAnnouncement {
  announcementSignature: string
  oraclePublicKey: string
  oracleEvent: {
    oracleNonces: string[]
    eventMaturityEpoch: number
    eventDescriptor: {
      digitDecompositionEvent: {
        base: number
        isSigned: boolean
        unit: string
        precision: number
        nbDigits: number
      }
    }
    eventId: EventId
  }
}

export interface PythiaAttestation {
  eventId: EventId
  signatures: string[]
  values: string[]
}

export type EventType = 'announcement' | 'attestation'
export type ExpiryChannel = 'ALL' | Expiry
export type PythiaChannelAnnouncement =
  `${AssetPair}${`/${ExpiryChannel}` | ''}/announcement`
export type PythiaChannelAttestation =
  `${AssetPair}${`/${ExpiryChannel}` | ''}/attestation`

export const getPythiaChannel = ({
  assetPair,
  type,
  expiry,
}: {
  assetPair: AssetPair
  type: EventType
  expiry?: ExpiryChannel
}): PythiaChannelAnnouncement | PythiaChannelAttestation => {
  if (expiry) {
    return `${assetPair}/${expiry}/${type}`
  }
  return `${assetPair}/${type}`
}

export type PythiaSubscriptionAnnouncement =
  `${AssetPair}${`/${Expiry}` | ''}/announcement`
export type PythiaSubscriptionAttestation =
  `${AssetPair}${`/${Expiry}` | ''}/attestation`

export const unpackPythiaSubscription = (
  channel: PythiaSubscriptionAnnouncement | PythiaSubscriptionAttestation
): { assetPair: AssetPair; type: EventType; expiry?: Expiry } => {
  const [assetPair, expiry_or_type, type_or_undefined] = channel.split('/')
  if (type_or_undefined) {
    return {
      assetPair: assetPair as AssetPair,
      type: type_or_undefined as EventType,
      expiry: expiry_or_type as Expiry,
    }
  }
  return {
    assetPair: assetPair as AssetPair,
    type: expiry_or_type as EventType,
    expiry: undefined,
  }
}

export type PythiaEvent<Subscription, Data> = {
  channel: Subscription
  data: Data
}
