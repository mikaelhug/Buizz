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
