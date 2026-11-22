-- Regions whose source asks per route type and per stop carry both lists on
-- the station row, comma separated. Swiss rows leave them NULL.
ALTER TABLE stations ADD COLUMN route_types TEXT;
ALTER TABLE stations ADD COLUMN stop_ids TEXT;
