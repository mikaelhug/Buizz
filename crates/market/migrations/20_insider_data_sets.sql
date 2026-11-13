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
