CREATE TEMP TABLE adopted_batch AS SELECT nextval('ingest_batch_id') AS id;

CREATE TEMP TABLE adopted_dividend AS SELECT symbol, ex_date, tiingo FROM corporate_action_check
WHERE action = 'cash_dividend' AND kept = 'alpaca' AND alpaca IS NOT NULL AND tiingo IS NOT NULL
    AND abs(alpaca - tiingo) > 0.01 * greatest(alpaca, tiingo);

INSERT INTO ingest_batch (id, source, dataset, created_at, rows)
SELECT id, 'tiingo', 'corporate_actions', now()::TIMESTAMP, (SELECT count(*) FROM adopted_dividend)
FROM adopted_batch WHERE EXISTS (SELECT 1 FROM adopted_dividend);

INSERT INTO corporate_action_revision SELECT c.*, b.id FROM corporate_action c
JOIN adopted_dividend d USING (symbol, ex_date), adopted_batch b
WHERE c.kind = 'cash_dividend' AND c.source = 'alpaca';

DELETE FROM corporate_action c WHERE c.kind = 'cash_dividend' AND c.source = 'alpaca'
    AND EXISTS (SELECT 1 FROM adopted_dividend d WHERE d.symbol = c.symbol AND d.ex_date = c.ex_date);

INSERT INTO corporate_action (symbol, ex_date, kind, rate, source, batch_id)
SELECT d.symbol, d.ex_date, 'cash_dividend', d.tiingo, 'tiingo', b.id FROM adopted_dividend d, adopted_batch b
ON CONFLICT DO NOTHING;

INSERT INTO corporate_action_check_revision SELECT c.*, b.id FROM corporate_action_check c
JOIN adopted_dividend d USING (symbol, ex_date), adopted_batch b WHERE c.action = 'cash_dividend';

UPDATE corporate_action_check SET kept = 'tiingo', batch_id = b.id FROM adopted_dividend d, adopted_batch b
WHERE corporate_action_check.symbol = d.symbol AND corporate_action_check.ex_date = d.ex_date
    AND corporate_action_check.action = 'cash_dividend';

DROP TABLE adopted_batch;
DROP TABLE adopted_dividend;

DELETE FROM coverage WHERE dataset = 'corporate_actions' AND source = 'alpaca' AND symbol <> '';

CREATE TEMP TABLE ended_key AS SELECT symbol AS key, split_part(symbol, '@', 1) AS ticker,
    CAST(split_part(symbol, '@', 2) AS DATE) AS last FROM instrument WHERE contains(symbol, '@');

CREATE TEMP TABLE earlier_company AS SELECT DISTINCT k.key, e.key AS earlier, e.last FROM ended_key k
JOIN ended_key e ON e.ticker = k.ticker AND e.last < k.last;

CREATE TEMP TABLE trimmed_batch AS SELECT nextval('ingest_batch_id') AS id;

CREATE TEMP TABLE trimmed_bar AS SELECT DISTINCT b.symbol, b.date FROM bar_1d b
JOIN earlier_company c ON b.symbol = c.key AND b.date <= c.last
JOIN bar_1d e ON e.symbol = c.earlier AND e.date = b.date AND e.open = b.open AND e.high = b.high
    AND e.low = b.low AND e.close = b.close AND e.volume = b.volume;

INSERT INTO ingest_batch (id, source, dataset, created_at, rows)
SELECT id, 'alpaca', 'bars1d', now()::TIMESTAMP, (SELECT count(*) FROM trimmed_bar) FROM trimmed_batch
WHERE EXISTS (SELECT 1 FROM trimmed_bar);

INSERT INTO bar_1d_revision SELECT b.*, t.id FROM bar_1d b JOIN trimmed_bar USING (symbol, date), trimmed_batch t;

DELETE FROM bar_1d b WHERE EXISTS (SELECT 1 FROM trimmed_bar t WHERE t.symbol = b.symbol AND t.date = b.date);

DELETE FROM quiet_session q WHERE EXISTS (SELECT 1 FROM earlier_company c
    JOIN quiet_session e ON e.symbol = c.earlier AND e.date = q.date
    WHERE c.key = q.symbol AND q.date <= c.last);

DROP TABLE trimmed_bar;
DROP TABLE trimmed_batch;
DROP TABLE earlier_company;
DROP TABLE ended_key;
