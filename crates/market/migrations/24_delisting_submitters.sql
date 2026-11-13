DELETE FROM delisting
WHERE outcome = 'no_ticker'
  AND accession IN (
    SELECT accession FROM sec_filing
    WHERE form IN ('25', '25-NSE')
    GROUP BY accession
    HAVING bool_or(cik = TRY_CAST(split_part(accession, '-', 1) AS UINTEGER))
       AND bool_or(cik IS DISTINCT FROM TRY_CAST(split_part(accession, '-', 1) AS UINTEGER))
  );
