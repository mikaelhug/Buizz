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
