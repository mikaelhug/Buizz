ALTER TABLE run ADD COLUMN origin TEXT NOT NULL DEFAULT 'request' CHECK (origin IN ('request', 'screening'));
