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
