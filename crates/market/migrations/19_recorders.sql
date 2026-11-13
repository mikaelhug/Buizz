CREATE TABLE IF NOT EXISTS option_quote (
    contract VARCHAR NOT NULL,
    underlying VARCHAR NOT NULL,
    expiration DATE NOT NULL,
    option_right VARCHAR NOT NULL,
    strike DECIMAL(18, 6) NOT NULL,
    session DATE NOT NULL,
    recorded_at TIMESTAMP NOT NULL,
    bid DECIMAL(18, 6),
    ask DECIMAL(18, 6),
    bid_size UBIGINT,
    ask_size UBIGINT,
    quoted_at TIMESTAMP,
    last DECIMAL(18, 6),
    traded_at TIMESTAMP,
    volume UBIGINT NOT NULL,
    open_interest UBIGINT,
    open_interest_date DATE,
    implied_volatility DECIMAL(18, 10),
    delta DECIMAL(18, 10),
    gamma DECIMAL(18, 10),
    theta DECIMAL(18, 10),
    vega DECIMAL(18, 10),
    rho DECIMAL(18, 10),
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS trading_halt (
    symbol VARCHAR NOT NULL,
    halted_at TIMESTAMP NOT NULL,
    name VARCHAR NOT NULL,
    market VARCHAR NOT NULL,
    reason VARCHAR NOT NULL,
    pause_threshold DECIMAL(18, 6),
    resumption_date DATE,
    quotes_resume_at TIMESTAMP,
    trading_resumes_at TIMESTAMP,
    recorded_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS etf_holding (
    etf VARCHAR NOT NULL,
    as_of DATE NOT NULL,
    name VARCHAR NOT NULL,
    ticker VARCHAR,
    identifier VARCHAR,
    weight DECIMAL(18, 6) NOT NULL,
    quantity DECIMAL(24, 6),
    recorded_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS etf_shares (
    etf VARCHAR NOT NULL,
    date DATE NOT NULL,
    nav DECIMAL(18, 6) NOT NULL,
    shares_outstanding DECIMAL(24, 6) NOT NULL,
    net_assets DECIMAL(24, 6) NOT NULL,
    recorded_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);
