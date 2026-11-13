DELETE FROM short_volume;

DELETE FROM coverage WHERE dataset = 'short_volume' AND source = 'finra';
