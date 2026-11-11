CREATE TABLE ledger_entry_next (
    run_id INTEGER NOT NULL REFERENCES run (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('deposit', 'fill', 'dividend', 'split', 'borrow_fee', 'liquidation', 'financing')),
    symbol TEXT,
    cash_delta TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (run_id, seq)
) STRICT;

INSERT INTO ledger_entry_next SELECT run_id, seq, at, kind, symbol, cash_delta, body FROM ledger_entry;

DROP TABLE ledger_entry;

ALTER TABLE ledger_entry_next RENAME TO ledger_entry;

CREATE INDEX ledger_fill_by_order ON ledger_entry (run_id, json_extract(body, '$.order_id'))
    WHERE kind = 'fill';
