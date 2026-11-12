CREATE TABLE corporate_action_next (
    symbol VARCHAR NOT NULL,
    ex_date DATE NOT NULL,
    kind VARCHAR NOT NULL CHECK (kind IN ('split', 'cash_dividend', 'stock_dividend', 'spin_off', 'cash_merger', 'stock_merger', 'worthless_removal', 'name_change')),
    other_symbol VARCHAR NOT NULL DEFAULT '',
    old_rate DECIMAL(18, 8),
    new_rate DECIMAL(18, 8),
    rate DECIMAL(18, 8),
    source VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (symbol, ex_date, kind, other_symbol)
);
INSERT INTO corporate_action_next BY NAME SELECT * FROM corporate_action;
DROP TABLE corporate_action;
ALTER TABLE corporate_action_next RENAME TO corporate_action;

CREATE TABLE corporate_action_revision_next AS SELECT *, CAST(NULL AS BIGINT) AS superseded_by FROM corporate_action LIMIT 0;
INSERT INTO corporate_action_revision_next BY NAME SELECT *, '' AS other_symbol FROM corporate_action_revision;
DROP TABLE corporate_action_revision;
ALTER TABLE corporate_action_revision_next RENAME TO corporate_action_revision;
ALTER TABLE corporate_action_revision ALTER COLUMN other_symbol SET DEFAULT '';
