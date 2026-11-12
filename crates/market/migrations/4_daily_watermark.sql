CREATE TABLE daily_watermark (
    id BOOLEAN PRIMARY KEY DEFAULT true CHECK (id),
    through TIMESTAMP NOT NULL
);
