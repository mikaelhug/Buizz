CREATE TABLE IF NOT EXISTS run (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bot TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('backtest', 'live')),
    status TEXT NOT NULL CHECK (status IN ('queued', 'starting', 'running', 'paused', 'completed', 'failed', 'cancelled')),
    desired TEXT NOT NULL CHECK (desired IN ('running', 'paused', 'stopped')),
    config TEXT NOT NULL,
    holdout INTEGER NOT NULL CHECK (holdout IN (0, 1)),
    bot_hash TEXT NOT NULL,
    engine_version TEXT NOT NULL,
    seed INTEGER NOT NULL,
    data_watermark INTEGER,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    step_seq INTEGER NOT NULL DEFAULT 0,
    clock INTEGER,
    equity TEXT,
    checkpoint BLOB,
    bot_state BLOB,
    result_hash TEXT,
    metrics TEXT,
    badges TEXT NOT NULL DEFAULT '[]',
    message TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS run_by_bot ON run (bot, created_at);
CREATE INDEX IF NOT EXISTS run_by_desired ON run (desired, status);
CREATE UNIQUE INDEX IF NOT EXISTS run_one_live_per_bot ON run (bot)
    WHERE mode = 'live' AND status IN ('queued', 'starting', 'running', 'paused');

CREATE TABLE IF NOT EXISTS run_order (
    run_id INTEGER NOT NULL REFERENCES run (id) ON DELETE CASCADE,
    order_id INTEGER NOT NULL,
    client_id TEXT NOT NULL,
    symbol TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('working', 'filled', 'cancelled', 'expired', 'rejected')),
    decided_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (run_id, order_id),
    UNIQUE (run_id, client_id)
) STRICT;

CREATE TABLE IF NOT EXISTS ledger_entry (
    run_id INTEGER NOT NULL REFERENCES run (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('deposit', 'fill', 'dividend', 'split', 'borrow_fee', 'liquidation')),
    symbol TEXT,
    cash_delta TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (run_id, seq)
) STRICT;

CREATE TABLE IF NOT EXISTS equity_point (
    run_id INTEGER NOT NULL REFERENCES run (id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    equity TEXT NOT NULL,
    cash TEXT NOT NULL,
    gross_exposure TEXT NOT NULL,
    PRIMARY KEY (run_id, at)
) STRICT;

CREATE TABLE IF NOT EXISTS run_event (
    run_id INTEGER NOT NULL REFERENCES run (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    clock INTEGER,
    kind TEXT NOT NULL CHECK (kind IN ('log', 'status', 'order_rejected', 'risk_breach', 'bot_exited', 'data_warning')),
    body TEXT NOT NULL,
    PRIMARY KEY (run_id, seq)
) STRICT;

CREATE TABLE IF NOT EXISTS job_run (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    outcome TEXT CHECK (outcome IN ('ok', 'noop', 'backoff', 'failed')),
    rows INTEGER NOT NULL DEFAULT 0,
    message TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS job_run_by_job ON job_run (job, started_at);

CREATE TABLE IF NOT EXISTS source_fetch (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    dataset TEXT NOT NULL,
    symbol TEXT,
    requested_from TEXT,
    requested_to TEXT,
    at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ok', 'empty', 'rate_limited', 'rejected', 'failed')),
    http_status INTEGER,
    rows INTEGER NOT NULL DEFAULT 0,
    retry_after INTEGER,
    message TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS source_fetch_by_key ON source_fetch (source, dataset, symbol, at);

CREATE INDEX IF NOT EXISTS source_fetch_rate_limits ON source_fetch (source, retry_after)
    WHERE status = 'rate_limited';
