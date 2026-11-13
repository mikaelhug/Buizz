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
