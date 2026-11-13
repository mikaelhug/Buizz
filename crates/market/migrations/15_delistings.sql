CREATE TABLE IF NOT EXISTS delisting (
    accession VARCHAR NOT NULL,
    ticker VARCHAR,
    listing VARCHAR,
    outcome VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);
