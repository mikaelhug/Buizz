CREATE TABLE fomc_meeting (
    date DATE NOT NULL,
    scheduled BOOLEAN NOT NULL,
    statement_at TIMESTAMP,
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (date, scheduled)
);
CREATE TABLE fomc_meeting_revision AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM fomc_meeting LIMIT 0;
