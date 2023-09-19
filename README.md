# pythia

![alt text](https://upload.wikimedia.org/wikipedia/commons/thumb/3/3d/Eug%C3%A8ne_Delacroix_-_Lycurgus_Consulting_the_Pythia_-_Google_Art_Project.jpg/2560px-Eug%C3%A8ne_Delacroix_-_Lycurgus_Consulting_the_Pythia_-_Google_Art_Project.jpg)

A numeric (and extensible) oracle implementation for bitcoin forked from sibyls, the oracle of lava.xyz.

## API Description

### Get supported assets

```sh
curl -X GET http://localhost:8000/v1/asset
```

This endpoint return all asset pairs supported of the running oracle instance.

### Get configuration

```sh
curl -X GET http://localhost:8000/v1/{asset_id}/config
```

This endpoint returns the [oracle config](#configure) for the asset pair specified with the asset_id that is returned by previous endpoint.

Output example:

```json
{
    "pricefeed": "lnmarkets",
    "announcement_offset": "1day",
    "frequency": "1min"
}
```

## Run

To run, first clone the repository and build:

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

Then, you can run pythia by executing:

```sh
./target/release/pythia
```
Pythia API uses port 8000 by default. You can specify another port in CLI. To specify another port, use `--port` argument in CLI:

```sh
./target/release/pythia --port <PORT>
```

If you use another database than default, do not forget to specify the postgres URL using `--postgres-url` argument:

```sh
./target/release/pythia --postgres-url <URL>
```

To specify a file to read the oracle secret key from, execute:

```sh
./target/release/pythia -s <FILE>
```

One is generated if not provided.

To specify a file to read asset pair configs from (more on this in [Asset Pairs](#asset-pairs)), execute:

```sh
./target/release/pythia -a <FILE>
```

One is expected at `config/asset_pair.json` if not provided.

To specify a file to read oracle configs from (more on this in [Configure](#configure)), execute:

```sh
./target/release/pythia -o <FILE>
```

One is expected at `config/oracle.json` if not provided.

For help, execute:

```sh
./target/release/pythia -h
```

For optional logging, you can run the above commands with the `RUST_LOG` environment variable set (see [`env_logger`](https://docs.rs/env_logger/0.9.0/env_logger/) for more), for example:

```sh
RUST_LOG=INFO ./target/release/pythia
```

Currently, the only logging done is at the `INFO` and `DEBUG` levels.

### Configure

Asset pair configs will be discussed in [Asset Pairs](#asset-pairs).

There are three configurable parameters for the oracle:

| name                  | type                                                                                                                                                                         | description                                                                                                           |
|-----------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------|
| `pricefeed`           | `(lnm\|bitstamp\|kraken\|gateio)`                                                                                                                                                | Source of the stream of price to attest                                                                               |
| `frequency`           | `(\d+(nsec\|ns\|usec\|us\|msec\|ms\|seconds\|second\|sec\|s\|minutes\|minute\|min\|m\|hours\|hour\|hr\|h\|days\|day\|d\|weeks\|week\|w\|months\|month\|M\|years\|year\|y))+` | frequency of attestation                                                                                              |
| `announcement_offset` | `(\d+(nsec\|ns\|usec\|us\|msec\|ms\|seconds\|second\|sec\|s\|minutes\|minute\|min\|m\|hours\|hour\|hr\|h\|days\|day\|d\|weeks\|week\|w\|months\|month\|M\|years\|year\|y))+` | offset from attestation for announcement, e.g. with an offset of `5h` announcements happen at `attestation_time - 5h` |

The program defaults are located in `config/oracle.json`.

## Extend

This oracle implementation is extensible to using other pricefeeds, asset pairs, and (to come) event descriptors (for more information, see https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#event-descriptor) rather than just {Bitstamp, Kraken, Gate.io, LNmarkets.com}, BTCUSD.

### Pricefeeds

Pricefeeds can be easily added as needed. To add a new pricefeed, say, Binance, you must implement the `oracle::pricefeeds::PriceFeed` trait. Note that you will have to implement `translate_asset_pair` for all possible variants of `AssetPair`, regardless of whether you use all of their announcements/attestations. Create `binance.rs` in the `src/oracle/pricefeeds` directory, implement it, and add the module `binance` in `src/oracle/mod.rs` and re-export it:

```rust
// snip
mod kraken;
mod binance; // <<

// snip
pub use kraken::Kraken;
pub use binance::Binance; // <<
```

Available `PriceFeedError` variants are in `src/oracle/pricefeeds/error.rs`. Then, add them in `ImplemetedPriceFeed` enum in `src/common.rs` and in `config/asset_ pair.json`.

After this, you are good to go!

### Asset Pairs

Asset pairs may also be added, although it is a bit more involved. To add a new asset pair, say, ETHUSD, you must first add an entry in `config/asset_pair.json`, or whatever file you are using for asset pair config. There, you will add an `AssetPairInfo` object to the outermost array. `AssetPairInfo`s contain the following fields:

| name               | type                                                                                                                      | description      |
|--------------------|---------------------------------------------------------------------------------------------------------------------------|------------------|
| `asset_pair`       | `AssetPair` enum                                                                                                          | asset pair       |
| `event_descriptor` | [`event_descriptor`](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#event-descriptor) | event descriptor |

For now, the only `event_descriptor` supported is `digit_decomposition_event_descriptor` because that is the most immediate use case (for bitcoin). However, `enum_event_descriptor` will be added in the future. Furthermore, note that because of a quirk in the encodings of attestations due to inconsistencies between encoding libraries and [DLC spec](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md), currently `event_descriptor.base` must be 2 (binary) or else decoding will be incorrect. This will be changed in the future.

An example of a valid addition in `config/asset_pair.json` is the following:

```json
[
    {
        "pricefeed": "bitstamp",
        "asset_pair": "ETHUSD",
        "event_descriptor": {
            "base": 2,
            "is_signed": false,
            "unit": "ETHUSD",
            "precision": 0,
            "num_digits": 14
        }
    },
]
```

Then, you must add a variant to `AssetPair` in `src/oracle/common.rs`:

```rust
// snip
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum AssetPair {
    BTCUSD,
    ETHUSD, // <<
}
// snip
```

and finally add match arms to **every** pricefeed in their implementation of the trait method `translate_asset_pair`, for example:

```rust
// snip
impl PriceFeed for Kraken {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::BTCUSD => "XXBTZUSD",
            AssetPair::ETHUSD => "XETHZUSD", // <<
        }
    }

    //snip
}
```
