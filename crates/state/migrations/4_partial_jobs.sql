CREATE TABLE job_run_next (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    outcome TEXT CHECK (outcome IN ('ok', 'partial', 'noop', 'backoff', 'failed')),
    rows INTEGER NOT NULL DEFAULT 0,
    message TEXT
) STRICT;

INSERT INTO job_run_next SELECT id, job, started_at, finished_at, outcome, rows, message FROM job_run;

DROP TABLE job_run;

ALTER TABLE job_run_next RENAME TO job_run;

CREATE INDEX job_run_by_job ON job_run (job, started_at);
