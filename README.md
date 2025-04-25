# Pythia

![alt text](https://upload.wikimedia.org/wikipedia/commons/thumb/3/3d/Eug%C3%A8ne_Delacroix_-_Lycurgus_Consulting_the_Pythia_-_Google_Art_Project.jpg/2560px-Eug%C3%A8ne_Delacroix_-_Lycurgus_Consulting_the_Pythia_-_Google_Art_Project.jpg) _Eugene Delacroix: Lycurgus consulting the Pythia_

A numeric (and extensible) oracle implementation forked from [sibyls](https://github.com/lava-xyz/sibyls), the oracle of lava.xyz.

## Features:

- Postgres database for persistence
- Configurable announcement and attestation schedule using cron expressions and an offset
- Support for multiple asset pairs in the same instance
- Supports multiple price feeds (currently LN Markets by default)
- Extensible architecture for adding new price feeds
- Websocket API to be notified and receive announcements/attestations when created
- REST API for querying historical data
- Support for efficiently batched queries of announcements
- Configurable via CLI arguments or environment variables
- Debug mode for testing: allow to force an attestation at any date and price through API.

Pythia chooses `eventId` of announcements and attestation to be of the form `{asset_pair_snake_case}{unix_maturity_timestamp_in_second}`. This has the advantage of allowing you to change the price feed for the same asset pair before attesting without issue but also means it will be inconvenient to run an instance of pythia which attests the same asset pair prices of several price feeds.

## Configuration

You can configure Pythia with the CLI arguments documented at `pythia --help` or environment variables.

Pythia will run an http server on port `8000` by default but you can change it with the `--port` argument or the `PORT` environment variable.

- `POSTGRES_PASSWORD`: The password for the PostgreSQL database.
- `POSTGRES_USER`: The username for the PostgreSQL database.
- `POSTGRES_DB`: The name of the PostgreSQL database.
- `POSTGRES_HOST`: The host address of the PostgreSQL server.
- `POSTGRES_PORT`: The port on which the PostgreSQL server is running.
- `PYTHIA_SECRET_KEY`: The secret key used for signing announcements and attestation.
- `PYTHIA_DEBUG_MODE`: If set to `true`, the server will run in debug mode.
- `PYTHIA_PORT`: The port on which the HTTP server will run.
- `PYTHIA_NB_CONNECTION`: The number of connection to the database (default to 10).
- `RUST_LOG`: Log level for the application. See [`env_logger`](https://docs.rs/env_logger/0.9.0/env_logger/) for more.
- `PORT`: The port on which the HTTP server will run.

The schedule of attestations and announcements is specified with a CRON formatted string for attestations times. The announcements times are automatically deduced from the specified offset duration between the publication of an announcement and its maturity.

The schedule must be given through the CLI with `--schedule` and `--offset` or a json config file which is assumed to be `config.json` by default but can be changed using CLI argument `--config-file`.

Notice that a schedule that generate a lot of attestation for the duration of the offset will have to produce a lot of announcement at the first boot of the oracle. This may take several minutes for all normally already published announcements to be available through the API.

## REST API usage

### API-JS

For more convenience, pythia ships with a typescript wrapper in a npm package which features all the endpoints below.

### Get supported assets

To return all asset pairs supported of the running oracle instance in a array:

```sh
curl -X GET http://localhost:8000/v1/assets
```

### Get configuration

To get the [scheduler config](#configure) and pricefeed source for the asset pair specified with the asset_id that is returned by previous endpoint.

```sh
curl -X GET http://localhost:8000/v1/{asset_id}/config
```

Notice that the schedule of announcement/attestation publication is the the same for all asset pairs of a running oracle.

Output example:

```json
{
  "pricefeed": "lnmarkets",
  "schedule": "0 */30 * * * * *",
  "announcement_offset": "1d"
}
```

### Get announcements

The oracle announcements are returned as serialized json of the [`rust-dlc oracle announcement struct`](https://docs.rs/dlc-messages/latest/dlc_messages/oracle_msgs/struct.OracleAnnouncement.html).
There are two API endpoints to get oracle announcements.

#### Individual announcement:

You can obtain the announcement for the specified asset at a give date using the following endpoint:

```sh
curl -X GET http://localhost:8000/v1/asset/{asset_id}/announcement/{date_in_rfc3339_format}
```

The asset pair is identified through its `asset_id` (each component of the pair in `snake_case`) for the date formatted using RFC 3339.

Request example:

```sh
curl -X GET http://localhost:8000/v1/asset/btc_usd/announcement/2024-01-22T16:28:00+01:00
```

Output:

```json
{
  "announcementSignature": "731c70e...48a6de3b840ac603c2704e11d",
  "oraclePublicKey": "b0d191ff8...276",
  "oracleEvent": {
    "oracleNonces": [
      "4e8a0e7fb0c77f10057f4fb693bad73a141cc8bea846a88bda9c04eed5b9898f",
      "...",
      "b5607c9d342dc3d518c38d08c54a148e419d93c771bd9ed63f5f2f90b652487a"
    ],
    "eventMaturityEpoch": 1705937280,
    "eventDescriptor": {
      "digitDecompositionEvent": {
        "base": 2,
        "isSigned": false,
        "unit": "usd/btc",
        "precision": 7,
        "nbDigits": 30
      }
    },
    "eventId": "btc_usd1705937280"
  }
}
```

#### Batch of announcements for an asset

Using the `http://localhost:8000/v1/asset/{asset_pair}/announcements/batch` endpoint, you can ask for all announcements for an asset pair at maturities specified with a POST request in a JSON array with key `maturities`:

```sh
curl -X POST http://localhost:8000/v1/asset/btc_usd/announcements/batch -d '{"maturities": ["2025-04-10T14:49:00Z","2025-04-10T14:50:00Z","2025-04-10T14:51:00Z","2025-04-10T14:52:00Z","2025-04-10T14:53:00Z"]}' -H "Content-Type: application/json"
```

If an announcement exists for each maturity, all the announcements are returned in chronological order (not the same order as maturities given !) in a JSON array. Caution: if any maturity has no associated announcement the request fails.

### Get attestation

There is generally no need to get more then one attestation of an asset pair to settle a DLC. So it is only possible to request attestation on at a time using the following endpoint:

```sh
curl -X GET http://localhost:8000/v1/asset/{asset_id}/attestation/{date_in_rfc3339_format}
```

It returns the [`oracle attestation`](https://docs.rs/dlc-messages/latest/dlc_messages/oracle_msgs/struct.OracleAttestation.html) for the asset pair identified through its `asset_id` (each component of the pair in `snake_case`) for the date formatted using RFC 3339.

The attestation response is slightly different of the JSON serialisation of rust-dlc struct. It contains the eventId, the signatures as array of hex strings and the outcomes as array of string but with key `"values"`.

Request example with the same event as announcement:

```sh
curl -X GET http://localhost:8000/v1/asset/btc_usd/attestation/2024-01-22T16:28:00+01:00
```

Output:

```json
{
  "eventId": "btc_usd1705937280",
  "signatures": [
    "4e8a0e7fb0c77f10057f4fb693bad73a141cc8bea846a88bda9c04eed5b9898fdf9504361c674a6578330840e621517492caff0abcf0984f2d0f6cd101e6b166",
    "...",
    "b5607c9d342dc3d518c38d08c54a148e419d93c771bd9ed63f5f2f90b652487a1aba6de3c4b048ee922621152bb94db7967d0024bf429002a0119612104a727f"
  ],
  "values": ["0", "...", "0"]
}
```

## Websocket usage

### Connection

Pythia API features a websocket which can be used to receive announcements and attestations when they are created by Pythia's scheduler or to query them. The websocket is located as `/v1/ws`. Using `wscat` you can connect to it as such:

```sh
wscat --connect localhost:8000/v1/ws
```

Upon connecting, the client is by default subscribed to the channel `btc_usd/attestation`. It receives attestations from the `BtcUsd` asset-pair.

Example of attestation received upon connecting to the websocket:

```json
< {
  "jsonrpc": "2.0",
  "method": "subscriptions",
  "params": {
    "channel": "btc_usd/attestation",
    "data": {
      "eventId": "btc_usd1705947180",
      "signatures": [
        "5c6041a980f...e1a3bd30",
        "...",
        "f614036fa15...15315183"
      ],
      "values": [
        "0",
        "...",
        "1"
      ]
    }
  }
}
```

### Subscribe/Unsubscribe

The websocket use [`JSON-RPC`](https://www.jsonrpc.org/specification) standard. You can subscribe or unsubscribe to a channel using the corresponding JSON-RPC methods with channel asset-pair and event type as params.

Example:

Opt-out to default subscription to `btc_usd/attestation` using the following RPC:

```json
> {
    "jsonrpc": "2.0",
    "method": "unsubscribe",
    "params":
    {
        "type": "attestation",
        "assetPair": "btc_usd",
    },
    "id": 1337
}
```

Websocket response, no new attestations are received after it:

```json
< {
    "jsonrpc": "2.0",
    "result": "Successfully unsubscribe for attestation of the btc_usd pair",
    "id": 1337
}
```

You can subscribe to the freshly prepared announcements using the following:

```json
> {
    "jsonrpc": "2.0",
    "method": "subscribe",
    "params":
    {
        "type": "announcement",
        "assetPair": "btc_usd",
    },
    "id": 4242
}
```

To which the websocket responds:

```json
< {
  "jsonrpc": "2.0",
  "result": "Successfully subscribe for announcement of the btc_usd pair",
  "id": 4242
}
```

and you will received the announcement as soon as they are scheduled through the websocket.

In the not frequent case where Pythia is started and must catch up missing announcements to create, those are not send to the websocket. Only announcements that Pythia must wait for are sent to the subscribed clients. However, all the attestations are sent to clients.

### Request specific announcement or attestation with the websocket

The websocket also feature `get` JSON-RPC methods to query for specific announcement or attestation that were already scheduled before. The method value is actually ignored, the websocket recognise your request using the `eventId` you must provide in `params`.

Example of request:

```json
> {
    "jsonrpc": "2.0",
    "method": "anything_but_prefer_get",
    "params":
    {
        "type": "announcement",
        "assetPair": "btc_usd",
        "eventId": "btc_usd1705947180",
    },
    "id": 21000000
}
```

Websocket answer:

```json
< {
  "jsonrpc": "2.0",
  "result": {
    "announcementSignature": "966adaa3ca3...e41",
    "oraclePublicKey": "b0d191...f5d276",
    "oracleEvent": {
      "oracleNonces": [
        "c6698e177...d89d2b88",
        "...",
        "8dc8da506b039...afe28f6"
      ],
      "eventMaturityEpoch": 1705948020,
      "eventDescriptor": {
        "digitDecompositionEvent": {
          "base": 2,
          "isSigned": false,
          "unit": "usd/btc",
          "precision": 7,
          "nbDigits": 30
        }
      },
      "eventId": "btc_usd1705948020"
    }
  },
  "id": 21000000
}
```

## Run Pythia

To run pythia, first clone the repository and build:

```sh
git clone https://github.com/ln-market/pythia.git
cd pythia
cargo build --release
```

Pythia uses PostgreSQL as DB backend so make sure it is installed on your system. Then install and use sqlx to create a dedicated postgres DB for your oracle instance:

```sh
cargo install sqlx-cli
sqlx database create
sqlx migrate run
```

This will create a database oracle in your running postgres server by default. You can change this by editing the `DATABASE_URL` value in the .env file of the repo before running the migrations.

For optional logging, you can run pythia with the `RUST_LOG` environment variable set (see [`env_logger`](https://docs.rs/env_logger/0.9.0/env_logger/) for more):

```sh
RUST_LOG=INFO ./target/release/pythia
```

Currently, the only logging done is at the `INFO`, `DEBUG` and `TRACE` levels.

## Run with docker

With docker engine started you can use the following commands to build the docker image and run it:

```sh
docker compose build
docker compose up
```

The API will run on port 8000.

### Scheduler configuration

Asset pair configs will be discussed in [Asset Pairs](#asset-pairs).

The schedule of announcement and attestation is defined by two configurable parameters:

| name                  | type                | description                                                                                                                      |
| --------------------- | ------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `schedule`            | `0 */1 * * * * *`   | attestation schedule using CRON syntax. You can use crontab.guru to edit the schedule easily (but you must add the second field) |
| `announcement_offset` | `(\d+(humantime))+` | offset from attestation for announcement, e.g. with an offset of `5h` announcements happen at `attestation_time - 5h`            |

where `(humantime)` is any unit of duration in `(seconds|second|sec|s|minutes|minute|mi\|m|hours|hour|hr|h|days|day|d|weeks|week|w|months|month|M|years|year|y)`

This is taken from `config.json` file if no other path is specified in CLI with `--config-file` and if CLI does not fully set these parameters.

## Extend

Disclaimer: most of this part of the documentation is similar to the original sibyls implementation.

Like sibyls, this oracle implementation is extensible to using other pricefeeds and asset pairs rather than just {Bitstamp, Kraken, Gate.io, LNmarkets.com}, BTCUSD.

### Pricefeeds

Pricefeeds can be easily added as needed. To add a new pricefeed, say, Binance, you must implement the `pricefeeds::PriceFeed` trait. Note that you will have to implement `translate_asset_pair` for all possible variants of `AssetPair`, regardless of whether you use all of their announcements/attestations. Create `binance.rs` in the `src/pricefeeds` directory, implement it, and add the module `binance` in `src/pricefeeds/mod.rs` and re-export it:

```rust
// snip
mod kraken;
mod binance; // <<

// snip
pub use kraken::Kraken;
pub use binance::Binance; // <<
```

Available `PriceFeedError` variants are in `src/pricefeeds/error.rs`. Then, add the variant in `ImplementedPriceFeed` enum in `src/pricefeed/mod.rs`, return the boxed trait-object when matching the variant in `get_pricefeed`. You can then use it in `config.json`.

After this, you are good to go!

### Asset Pairs

Asset pairs may also be added, although it is a bit more involved. To add a new asset pair, say, ETHBTC, you must first add an entry in `"asset_pair_infos"` section of `config.json`, or whatever file you are using for asset pair config. There, you will add an `AssetPairInfo` object to the array. `AssetPairInfo`s contain the following fields:

| name               | type                                                                                                          | description                             |
| ------------------ | ------------------------------------------------------------------------------------------------------------- | --------------------------------------- |
| `pricefeed`        | `(lnmarkets\|bitstamp\|kraken\|gateio)`                                                                       | Source of the stream of price to attest |
| `asset_pair`       | `AssetPair` enum                                                                                              | asset pair                              |
| `event_descriptor` | [`event_descriptor`](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#event-descriptor) | event descriptor                        |

For now, the only `event_descriptor` supported is `digit_decomposition_event_descriptor` because that is the most immediate use case (for bitcoin).

An example of a valid addition in `config/asset_pair.json` is the following:

```json
[
  {
    "pricefeed": "bitstamp",
    "asset_pair": "eth_btc",
    "event_descriptor": {
      "base": 2,
      "is_signed": false,
      "unit": "eth/btc",
      "precision": 0,
      "num_digits": 14
    }
  }
]
```

Then, you must add a variant to `AssetPair` in `src/config/mod.rs`:

```rust
// snip
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum AssetPair {
    BtcUsd,
    EthBtc, // <<
}
// snip
```

and finally add match arms to **every** pricefeed in their implementation of the trait method `translate_asset_pair`, for example:

```rust
// snip
impl PriceFeed for Kraken {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::BtcUsd => "XXBTZUSD",
            AssetPair::EthBtc => "XETHZBTC", // <<
        }
    }

    //snip
}
```
