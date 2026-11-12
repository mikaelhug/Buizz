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
