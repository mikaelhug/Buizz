UPDATE run SET badges = json_insert(badges, '$[#]', 'drawdown_stop')
WHERE status = 'failed' AND message = 'stopped by drawdown limit';
