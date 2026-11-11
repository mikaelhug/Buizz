CREATE SEQUENCE IF NOT EXISTS ingest_batch_id START 1;

CREATE TABLE IF NOT EXISTS ingest_batch (
    id BIGINT PRIMARY KEY DEFAULT nextval('ingest_batch_id'),
    source VARCHAR NOT NULL,
    dataset VARCHAR NOT NULL,
    created_at TIMESTAMP NOT NULL,
    rows UBIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS instrument (
    symbol VARCHAR PRIMARY KEY,
    name VARCHAR NOT NULL,
    exchange VARCHAR NOT NULL,
    asset_class VARCHAR NOT NULL CHECK (asset_class IN ('stock', 'etf')),
    active BOOLEAN NOT NULL,
    shortable BOOLEAN NOT NULL,
    first_session DATE,
    last_session DATE,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS calendar (
    date DATE PRIMARY KEY,
    open TIMESTAMP NOT NULL,
    close TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS bar_1d (
    symbol VARCHAR NOT NULL,
    date DATE NOT NULL,
    open DECIMAL(18, 6) NOT NULL,
    high DECIMAL(18, 6) NOT NULL,
    low DECIMAL(18, 6) NOT NULL,
    close DECIMAL(18, 6) NOT NULL,
    volume UBIGINT NOT NULL,
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, date)
);

CREATE TABLE IF NOT EXISTS bar_1m (
    symbol VARCHAR NOT NULL,
    start TIMESTAMP NOT NULL,
    open DECIMAL(18, 6) NOT NULL,
    high DECIMAL(18, 6) NOT NULL,
    low DECIMAL(18, 6) NOT NULL,
    close DECIMAL(18, 6) NOT NULL,
    volume UBIGINT NOT NULL,
    feed VARCHAR NOT NULL CHECK (feed IN ('sip', 'iex')),
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS corporate_action (
    symbol VARCHAR NOT NULL,
    ex_date DATE NOT NULL,
    kind VARCHAR NOT NULL CHECK (kind IN ('split', 'cash_dividend')),
    old_rate DECIMAL(18, 8),
    new_rate DECIMAL(18, 8),
    rate DECIMAL(18, 8),
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, ex_date, kind)
);

CREATE TABLE IF NOT EXISTS fundamental_fact (
    symbol VARCHAR NOT NULL,
    cik UINTEGER NOT NULL,
    taxonomy VARCHAR NOT NULL,
    concept VARCHAR NOT NULL,
    unit VARCHAR NOT NULL,
    metric VARCHAR,
    period_type VARCHAR CHECK (period_type IN ('FY', 'Q')),
    period_start DATE,
    period_end DATE NOT NULL,
    value DECIMAL(38, 6) NOT NULL,
    fiscal_year INTEGER,
    fiscal_period VARCHAR,
    form VARCHAR NOT NULL,
    accession VARCHAR NOT NULL,
    filed DATE NOT NULL,
    frame VARCHAR,
    available_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS rate_daily (
    date DATE PRIMARY KEY,
    percent DECIMAL(8, 4) NOT NULL,
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS coverage (
    dataset VARCHAR NOT NULL,
    symbol VARCHAR NOT NULL,
    source VARCHAR NOT NULL,
    first DATE NOT NULL,
    last DATE NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    PRIMARY KEY (dataset, symbol, source)
);
