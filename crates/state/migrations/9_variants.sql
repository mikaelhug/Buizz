ALTER TABLE run ADD COLUMN variant TEXT GENERATED ALWAYS AS (json_extract(config, '$.params')) VIRTUAL;
ALTER TABLE run ADD COLUMN source_run INTEGER REFERENCES run (id);

DROP INDEX IF EXISTS run_one_live_per_bot;

CREATE UNIQUE INDEX run_one_live_per_variant ON run (bot, variant)
    WHERE mode = 'live' AND status IN ('queued', 'starting', 'running', 'paused');

CREATE INDEX run_by_variant ON run (bot, variant, mode, status);
