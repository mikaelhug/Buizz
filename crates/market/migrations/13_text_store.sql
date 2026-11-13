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
