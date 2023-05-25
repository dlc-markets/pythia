CREATE SCHEMA IF NOT EXISTS oracle;

CREATE TABLE IF NOT EXISTS oracle.events (
  -- ID of the event (e.g. BTCUSD165000000)
  id VARCHAR(32) PRIMARY KEY NOT NULL,
  -- Number of digits of the outcome
  digits INTEGER NOT NULL,
  -- number of digits used for precision
  precision INTEGER NOT NULL,
  -- Planned date of the attestation
  maturity TIMESTAMP WITH TIME ZONE NOT NULL,
  announcement_signature BIT(512) NOT NULL,
  outcome INTEGER
);

CREATE TABLE IF NOT EXISTS oracle.digits (
  -- Event
  event_id VARCHAR(32) NOT NULL,
  digit_index INTEGER NOT NULL,

  -- Nonce
  nonce_public BIT(256) NOT NULL,
  nonce_secret BIT(256),

  -- Outcome
  bit BIT(1),
  signature BIT(256),
  signing_ts TIMESTAMP WITH TIME ZONE,

  PRIMARY KEY (event_id, digit_index),
  FOREIGN KEY (event_id) REFERENCES oracle.events (id) ON UPDATE CASCADE ON DELETE CASCADE
);
