ALTER TABLE index_member ADD COLUMN IF NOT EXISTS article VARCHAR;
ALTER TABLE index_member_revision ADD COLUMN IF NOT EXISTS article VARCHAR;

CREATE TABLE IF NOT EXISTS fails_to_deliver (
    symbol VARCHAR NOT NULL,
    settlement DATE NOT NULL,
    cusip VARCHAR NOT NULL,
    quantity UBIGINT NOT NULL,
    price DECIMAL(18, 6),
    published DATE NOT NULL,
    downloaded DATE NOT NULL,
    file VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS article_title (
    article VARCHAR NOT NULL,
    title VARCHAR NOT NULL,
    batch_id BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS page_views (
    title VARCHAR NOT NULL,
    date DATE NOT NULL,
    views UBIGINT NOT NULL,
    published DATE NOT NULL,
    batch_id BIGINT NOT NULL
);
