-- The nearby query ranges over lat and lon together, which the composite
-- index covers. Mode is filtered in Rust after the query, not in SQL.
-- Every index multiplies D1 rows_written, so keep only the one in use.
DROP INDEX idx_stations_lat;
DROP INDEX idx_stations_lon;
DROP INDEX idx_stations_mode;
