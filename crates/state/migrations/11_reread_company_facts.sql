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

DELETE FROM source_fetch WHERE source = 'edgar' AND dataset = 'fundamentals';
