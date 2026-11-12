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
