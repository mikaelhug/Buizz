CREATE TABLE IF NOT EXISTS nordic_listing (
    source VARCHAR NOT NULL,
    listing VARCHAR NOT NULL,
    mic VARCHAR,
    isin VARCHAR,
    symbol VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    currency VARCHAR NOT NULL,
    sector VARCHAR,
    recorded_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS nordic_listing_seen (
    source VARCHAR NOT NULL,
    listing VARCHAR NOT NULL,
    date DATE NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS nordic_bar_1d (
    source VARCHAR NOT NULL,
    listing VARCHAR NOT NULL,
    date DATE NOT NULL,
    currency VARCHAR NOT NULL,
    open DECIMAL(18, 6) NOT NULL,
    high DECIMAL(18, 6) NOT NULL,
    low DECIMAL(18, 6) NOT NULL,
    close DECIMAL(18, 6) NOT NULL,
    volume UBIGINT,
    recorded_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS nordic_split (
    source VARCHAR NOT NULL,
    listing VARCHAR NOT NULL,
    date DATE NOT NULL,
    ratio DECIMAL(18, 6) NOT NULL,
    detected_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS fx_rate (
    currency VARCHAR NOT NULL,
    date DATE NOT NULL,
    per_euro DECIMAL(18, 6) NOT NULL,
    recorded_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);
