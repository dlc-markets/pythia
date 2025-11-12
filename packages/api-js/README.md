# Pythia API JavaScript Client

A JavaScript client for the Pythia API that provides access to oracle announcements and attestations.

## Installation

```bash
npm install @dlc-markets/pythia
```

## Usage

### Basic Setup

```javascript
import { Pythia } from "@dlc-markets/pythia";

const pythia = new Pythia({
  url: "http://localhost:8000", // Optional, defaults to process.env.PYTHIA_URL or http://localhost:8000
  version: "v1", // Optional, defaults to 'v1'
});
```

### Parsing Event IDs

The `parseEventId` function can decompose EventIds into their components:

```javascript
import { parseEventId } from "@dlc-markets/pythia";

// Parse a Spot EventId (format: {asset_pair}{unix_timestamp})
const spotResult = parseEventId("btc_usd1706272800");
console.log(spotResult);
// Output:
// {
//   assetPair: 'btc_usd',
//   time: Date(2024-01-26T08:00:00.000Z),
//   expiry: undefined
// }

// Parse a Forward EventId (format: {asset_pair}_{expiry}f{unix_timestamp})
const forwardResult = parseEventId("btc_usd_25DEC24f1706272800");
console.log(forwardResult);
// Output:
// {
//   assetPair: 'btc_usd',
//   expiry: '25DEC24',
//   time: Date(2024-01-26T08:00:00.000Z)
// }

// Parse a Delivery EventId (format: {asset_pair}_{expiry}d)
const deliveryResult = parseEventId("btc_usd_25DEC24d");
console.log(deliveryResult);
// Output:
// {
//   assetPair: 'btc_usd',
//   expiry: '25DEC24',
//   time: Date(2024-12-25T08:00:00.000Z) // Expiry date at 8 AM UTC
// }
```

### Type Assertions

The library provides type assertion functions to validate and convert strings to the proper types:

```javascript
import { assertExpiry, assertEventId } from "@dlc-markets/pythia";

// Validate and convert string to Expiry type
const expiry = assertExpiry("25DEC24"); // Returns '25DEC24' as Expiry type
// Throws error for invalid formats like '25DEC', '32JAN25', etc.

// Validate and convert string to EventId type
const eventId = assertEventId("btc_usd_25DEC24f1706272800"); // Returns as EventId type
// Throws error for invalid formats or unreasonable timestamps
```

### EventId Types

Pythia supports three types of EventIds:

1. **Spot**: `{asset_pair}{unix_timestamp}` - For immediate price feeds
2. **Forward**: `{asset_pair}_{expiry}f{unix_timestamp}` - For forward contracts with expiry dates
3. **Delivery**: `{asset_pair}_{expiry}d` - For delivery contracts at expiry

### API Methods

```javascript
// Get oracle public key
const { publicKey } = await pythia.getOraclePublicKey();

// Get available assets
const assets = await pythia.getAssets();

// Get asset configuration
const config = await pythia.getAsset({ assetPair: "btc_usd" });

// Get announcement by time and optional expiry
const announcement = await pythia.getAnnouncement({
  assetPair: "btc_usd",
  time: new Date("2024-01-26T08:00:00.000Z"),
  expiry: "25DEC24", // Optional
});

// Get announcement by EventId
const announcement = await pythia.getAnnouncementByEventId({
  eventId: "btc_usd_25DEC24f1706272800",
});

// Get attestation by time
const attestation = await pythia.getAttestation({
  assetPair: "btc_usd",
  time: new Date("2024-01-26T08:00:00.000Z"),
});

// Get attestation by EventId
const attestation = await pythia.getAttestationByEventId({
  eventId: "btc_usd1706272800",
});
```

### WebSocket Connection

```javascript
// Connect to WebSocket
await pythia.connect();

// Listen for events
pythia.on("connected", () => {
  console.log("Connected to Pythia WebSocket");
});

pythia.on("disconnected", () => {
  console.log("Disconnected from Pythia WebSocket");
});

// Listen for specific asset pair events
pythia.on("btc_usd/attestation", (attestation) => {
  console.log("Received attestation:", attestation);
});

pythia.on("btc_usd/announcement", (announcement) => {
  console.log("Received announcement:", announcement);
});

// Disconnect
pythia.disconnect();
```
