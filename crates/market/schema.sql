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

CREATE TABLE IF NOT EXISTS quiet_session (
    symbol VARCHAR NOT NULL,
    date DATE NOT NULL,
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
    kind VARCHAR NOT NULL CHECK (kind IN ('split', 'cash_dividend', 'stock_dividend', 'spin_off', 'cash_merger', 'stock_merger', 'worthless_removal', 'name_change')),
    other_symbol VARCHAR NOT NULL DEFAULT '',
    old_rate DECIMAL(18, 8),
    new_rate DECIMAL(18, 8),
    rate DECIMAL(18, 8),
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, ex_date, kind, other_symbol)
);

CREATE TABLE IF NOT EXISTS corporate_action_check (
    symbol VARCHAR NOT NULL,
    ex_date DATE NOT NULL,
    action VARCHAR NOT NULL CHECK (action IN ('split', 'cash_dividend')),
    alpaca DECIMAL(18, 8),
    tiingo DECIMAL(18, 8),
    kept VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, ex_date, action)
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

CREATE TABLE IF NOT EXISTS macro_observation (
    series VARCHAR NOT NULL,
    date DATE NOT NULL,
    published DATE NOT NULL,
    value DECIMAL(18, 6),
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (series, date, published)
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

CREATE TABLE IF NOT EXISTS company (
    symbol VARCHAR PRIMARY KEY,
    cik UINTEGER NOT NULL,
    sic USMALLINT,
    sic_description VARCHAR,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS earnings_release (
    symbol VARCHAR NOT NULL,
    cik UINTEGER NOT NULL,
    accession VARCHAR NOT NULL,
    form VARCHAR NOT NULL,
    filed DATE NOT NULL,
    report_date DATE,
    accepted_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, accession)
);

CREATE TABLE IF NOT EXISTS index_member (
    index_name VARCHAR NOT NULL,
    symbol VARCHAR NOT NULL,
    start_date DATE,
    end_date DATE,
    batch_id BIGINT NOT NULL,
    article VARCHAR
);

CREATE TABLE IF NOT EXISTS index_history (
    index_name VARCHAR PRIMARY KEY,
    changes UINTEGER NOT NULL,
    reliable_from DATE,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS borrow_availability (
    symbol VARCHAR NOT NULL,
    recorded_at TIMESTAMP NOT NULL,
    con_id UBIGINT NOT NULL,
    rebate_rate DECIMAL(12, 6),
    fee_rate DECIMAL(12, 6),
    available UBIGINT NOT NULL,
    available_or_more BOOLEAN NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE instrument_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM instrument LIMIT 0;
CREATE TABLE calendar_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM calendar LIMIT 0;
CREATE TABLE bar_1d_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM bar_1d LIMIT 0;
CREATE TABLE corporate_action_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM corporate_action LIMIT 0;
ALTER TABLE corporate_action_revision ALTER COLUMN other_symbol SET DEFAULT '';
CREATE TABLE corporate_action_check_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM corporate_action_check LIMIT 0;
CREATE TABLE rate_daily_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM rate_daily LIMIT 0;
CREATE TABLE macro_observation_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM macro_observation LIMIT 0;
CREATE TABLE company_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM company LIMIT 0;
CREATE TABLE earnings_release_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM earnings_release LIMIT 0;
CREATE TABLE index_member_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM index_member LIMIT 0;
CREATE TABLE index_history_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM index_history LIMIT 0;

CREATE TABLE sec_filing (
    cik UINTEGER NOT NULL,
    accession VARCHAR NOT NULL,
    form VARCHAR NOT NULL,
    filed DATE NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (cik, accession)
);

CREATE TABLE daily_watermark (
    id BOOLEAN PRIMARY KEY DEFAULT true CHECK (id),
    through TIMESTAMP NOT NULL
);

CREATE TABLE insider_filing (
    symbol VARCHAR NOT NULL,
    issuer_cik UINTEGER NOT NULL,
    accession VARCHAR NOT NULL,
    form VARCHAR NOT NULL,
    filed DATE NOT NULL,
    accepted_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, accession)
);
CREATE TABLE insider_owner (
    symbol VARCHAR NOT NULL,
    accession VARCHAR NOT NULL,
    owner_cik UINTEGER NOT NULL,
    name VARCHAR NOT NULL,
    director BOOLEAN NOT NULL,
    officer BOOLEAN NOT NULL,
    ten_percent_owner BOOLEAN NOT NULL,
    other BOOLEAN NOT NULL,
    officer_title VARCHAR,
    batch_id BIGINT NOT NULL
);
CREATE TABLE insider_transaction (
    symbol VARCHAR NOT NULL,
    accession VARCHAR NOT NULL,
    line UINTEGER NOT NULL,
    security VARCHAR NOT NULL,
    date DATE NOT NULL,
    code VARCHAR NOT NULL,
    acquired BOOLEAN NOT NULL,
    shares DECIMAL(38, 6),
    price DECIMAL(38, 6),
    shares_after DECIMAL(38, 6),
    direct BOOLEAN NOT NULL,
    batch_id BIGINT NOT NULL
);
CREATE TABLE news_article (
    id UBIGINT NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    created_at TIMESTAMP NOT NULL,
    symbols VARCHAR[] NOT NULL,
    headline VARCHAR NOT NULL,
    summary VARCHAR NOT NULL,
    source VARCHAR NOT NULL,
    received_at TIMESTAMP NOT NULL,
    backfilled BOOLEAN NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (id, updated_at)
);
CREATE TABLE fomc_meeting (
    date DATE NOT NULL,
    scheduled BOOLEAN NOT NULL,
    statement_at TIMESTAMP,
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (date, scheduled)
);
CREATE TABLE fomc_meeting_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM fomc_meeting LIMIT 0;

CREATE TABLE earnings_insight (
    report VARCHAR PRIMARY KEY,
    issue_date DATE NOT NULL,
    published DATE NOT NULL,
    url VARCHAR NOT NULL,
    retrieved_from VARCHAR NOT NULL,
    digest VARCHAR NOT NULL,
    text VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE earnings_insight_fact (
    report VARCHAR NOT NULL,
    metric VARCHAR NOT NULL,
    scope VARCHAR NOT NULL,
    period VARCHAR NOT NULL,
    as_of DATE,
    value DECIMAL(18, 4) NOT NULL,
    unit VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (report, metric, scope, period)
);

CREATE TABLE earnings_insight_fact_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM earnings_insight_fact LIMIT 0;
CREATE TABLE IF NOT EXISTS current_report (
    symbol VARCHAR NOT NULL,
    cik UINTEGER NOT NULL,
    accession VARCHAR NOT NULL,
    form VARCHAR NOT NULL,
    filed DATE NOT NULL,
    report_date DATE,
    items VARCHAR[] NOT NULL,
    accepted_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, accession)
);
CREATE TABLE IF NOT EXISTS press_release (
    accession VARCHAR PRIMARY KEY,
    cik UINTEGER NOT NULL,
    accepted_at TIMESTAMP NOT NULL,
    document VARCHAR,
    exhibit VARCHAR,
    text VARCHAR,
    batch_id BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS insider_plan (
    symbol VARCHAR NOT NULL,
    accession VARCHAR NOT NULL,
    plan_10b5_1 BOOLEAN,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, accession)
);

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

CREATE SEQUENCE IF NOT EXISTS text_file_id START 1;

CREATE TABLE IF NOT EXISTS text_volume (
    id VARCHAR PRIMARY KEY,
    path VARCHAR NOT NULL,
    seen_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS text_file (
    id BIGINT PRIMARY KEY,
    volume VARCHAR NOT NULL,
    path VARCHAR NOT NULL,
    source VARCHAR NOT NULL,
    documents UINTEGER NOT NULL,
    bytes UBIGINT NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS filing_text (
    accession VARCHAR PRIMARY KEY,
    cik UINTEGER NOT NULL,
    form VARCHAR NOT NULL,
    accepted_at TIMESTAMP NOT NULL,
    retrieved_at TIMESTAMP NOT NULL,
    documents UINTEGER NOT NULL,
    failure VARCHAR,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS text_document (
    doc_id VARCHAR NOT NULL,
    content_hash VARCHAR NOT NULL,
    source VARCHAR NOT NULL,
    kind VARCHAR NOT NULL,
    accession VARCHAR,
    sequence UINTEGER,
    document VARCHAR NOT NULL,
    description VARCHAR,
    cik UINTEGER,
    published_at TIMESTAMP NOT NULL,
    retrieved_at TIMESTAMP NOT NULL,
    raw_bytes UBIGINT NOT NULL,
    text_chars UBIGINT,
    file_id BIGINT NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (doc_id, content_hash)
);

CREATE TABLE IF NOT EXISTS periodic_report (
    symbol VARCHAR NOT NULL,
    cik UINTEGER NOT NULL,
    accession VARCHAR NOT NULL,
    form VARCHAR NOT NULL,
    filed DATE NOT NULL,
    report_date DATE,
    accepted_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, accession)
);

CREATE TABLE IF NOT EXISTS section (
    doc_id VARCHAR NOT NULL,
    content_hash VARCHAR NOT NULL,
    splitter_version UINTEGER NOT NULL,
    ordinal UINTEGER NOT NULL,
    key VARCHAR,
    start_char UBIGINT,
    end_char UBIGINT,
    failure VARCHAR,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (doc_id, content_hash, splitter_version, ordinal)
);

CREATE TABLE IF NOT EXISTS delisting (
    accession VARCHAR NOT NULL,
    ticker VARCHAR,
    listing VARCHAR,
    outcome VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS recipe_version (
    recipe VARCHAR NOT NULL,
    version UINTEGER NOT NULL,
    schema VARCHAR NOT NULL,
    producer VARCHAR NOT NULL,
    prompt_hash VARCHAR,
    scope VARCHAR NOT NULL,
    state VARCHAR NOT NULL,
    gold VARCHAR,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS fact (
    recipe VARCHAR NOT NULL,
    version UINTEGER NOT NULL,
    doc_id VARCHAR NOT NULL,
    section_key VARCHAR NOT NULL,
    ordinal UINTEGER NOT NULL,
    accession VARCHAR,
    cik UINTEGER,
    status VARCHAR NOT NULL,
    fields VARCHAR,
    evidence VARCHAR,
    evidence_start UBIGINT,
    reason VARCHAR,
    published_at TIMESTAMP NOT NULL,
    produced_at TIMESTAMP NOT NULL,
    input_hash VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (recipe, version, doc_id, section_key, ordinal)
);

CREATE TABLE IF NOT EXISTS model_batch (
    recipe VARCHAR NOT NULL,
    version UINTEGER NOT NULL,
    remote_id VARCHAR NOT NULL,
    state VARCHAR NOT NULL,
    requests UINTEGER NOT NULL,
    failed UINTEGER,
    reserved_usd DECIMAL(18, 6) NOT NULL,
    recorded_at TIMESTAMP NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS model_request (
    remote_id VARCHAR NOT NULL,
    custom_id VARCHAR NOT NULL,
    doc_id VARCHAR NOT NULL,
    section_key VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (remote_id, custom_id)
);

CREATE TABLE IF NOT EXISTS model_spend (
    recipe VARCHAR NOT NULL,
    version UINTEGER NOT NULL,
    spent_at TIMESTAMP NOT NULL,
    calls UINTEGER NOT NULL,
    input_tokens UBIGINT NOT NULL,
    output_tokens UBIGINT NOT NULL,
    usd DECIMAL(18, 6) NOT NULL,
    batch_id BIGINT NOT NULL
);
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

CREATE TABLE IF NOT EXISTS insider_quarter (
    quarter DATE NOT NULL,
    reports UINTEGER NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (quarter)
);

CREATE TABLE IF NOT EXISTS insider_report (
    accession VARCHAR NOT NULL,
    issuer_cik UINTEGER NOT NULL,
    plan_10b5_1 BOOLEAN,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (accession)
);

CREATE TABLE IF NOT EXISTS insider_report_owner (
    accession VARCHAR NOT NULL,
    owner_cik UINTEGER NOT NULL,
    name VARCHAR NOT NULL,
    director BOOLEAN NOT NULL,
    officer BOOLEAN NOT NULL,
    ten_percent_owner BOOLEAN NOT NULL,
    other BOOLEAN NOT NULL,
    officer_title VARCHAR,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS insider_report_transaction (
    accession VARCHAR NOT NULL,
    line UINTEGER NOT NULL,
    security VARCHAR NOT NULL,
    date DATE NOT NULL,
    code VARCHAR NOT NULL,
    acquired BOOLEAN NOT NULL,
    shares DECIMAL(38, 6),
    price DECIMAL(38, 6),
    shares_after DECIMAL(38, 6),
    direct BOOLEAN NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS fails_to_deliver (
    symbol VARCHAR NOT NULL,
    settlement DATE NOT NULL,
    cusip VARCHAR NOT NULL,
    quantity UBIGINT NOT NULL,
    price DECIMAL(18, 6),
    published DATE NOT NULL,
    downloaded DATE NOT NULL,
    file VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS article_title (
    article VARCHAR NOT NULL,
    title VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS page_views (
    title VARCHAR NOT NULL,
    date DATE NOT NULL,
    views UBIGINT NOT NULL,
    published DATE NOT NULL,
    batch_id BIGINT NOT NULL
);

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
