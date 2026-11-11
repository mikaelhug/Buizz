CREATE TABLE sec_filing (
    cik UINTEGER NOT NULL,
    accession VARCHAR NOT NULL,
    form VARCHAR NOT NULL,
    filed DATE NOT NULL,
    batch_id BIGINT NOT NULL,
    PRIMARY KEY (cik, accession)
);
