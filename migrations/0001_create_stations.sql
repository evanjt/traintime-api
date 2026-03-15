CREATE TABLE stations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    mode TEXT NOT NULL
);

CREATE INDEX idx_stations_lat ON stations(lat);
CREATE INDEX idx_stations_lon ON stations(lon);
CREATE INDEX idx_stations_lat_lon ON stations(lat, lon);
CREATE INDEX idx_stations_mode ON stations(mode);
