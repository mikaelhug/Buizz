CREATE TABLE IF NOT EXISTS short_interest (
    symbol VARCHAR NOT NULL,
    settlement DATE NOT NULL,
    shares UBIGINT NOT NULL,
    previous_shares UBIGINT NOT NULL,
    average_daily_volume UBIGINT NOT NULL,
    days_to_cover DECIMAL(18, 6) NOT NULL,
    split BOOLEAN NOT NULL,
    revised BOOLEAN NOT NULL,
    published DATE NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS short_volume (
    symbol VARCHAR NOT NULL,
    date DATE NOT NULL,
    short_volume DECIMAL(18, 6) NOT NULL,
    short_exempt_volume DECIMAL(18, 6) NOT NULL,
    total_volume DECIMAL(18, 6) NOT NULL,
    batch_id BIGINT NOT NULL
);
