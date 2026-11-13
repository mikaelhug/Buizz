CREATE TEMP TABLE quiet_batch AS SELECT nextval('ingest_batch_id') AS id;

CREATE TEMP TABLE quiet_bar AS SELECT symbol, date FROM bar_1d WHERE source = 'tiingo' AND volume = 0;

INSERT INTO ingest_batch (id, source, dataset, created_at, rows)
SELECT id, 'tiingo', 'bars1d', now()::TIMESTAMP, (SELECT count(*) FROM quiet_bar) FROM quiet_batch
WHERE EXISTS (SELECT 1 FROM quiet_bar);

INSERT INTO quiet_session SELECT q.symbol, q.date, 'tiingo', b.id FROM quiet_bar q, quiet_batch b
ON CONFLICT DO NOTHING;

INSERT INTO bar_1d_revision SELECT d.*, b.id FROM bar_1d d JOIN quiet_bar q USING (symbol, date), quiet_batch b;

DELETE FROM bar_1d d WHERE EXISTS (SELECT 1 FROM quiet_bar q WHERE q.symbol = d.symbol AND q.date = d.date);

DROP TABLE quiet_batch;
DROP TABLE quiet_bar;
