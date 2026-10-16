-- Symbol packages (.snupkg): one row per version, one row per portable PDB.
-- PDBs are stored as individual objects so debuggers can fetch them by key.
CREATE TABLE symbol_packages (
    package_id TEXT NOT NULL,
    version TEXT NOT NULL,
    pushed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (package_id, version),
    FOREIGN KEY (package_id, version) REFERENCES package_versions(package_id, version)
);
CREATE TABLE symbol_files (
    package_id TEXT NOT NULL,
    version TEXT NOT NULL,
    file_name TEXT NOT NULL CHECK (file_name = lower(file_name)),
    symbol_key TEXT NOT NULL CHECK (symbol_key ~ '^[0-9a-f]{40}$'),
    storage_key TEXT NOT NULL UNIQUE,
    size BIGINT NOT NULL CHECK (size > 0),
    sha256 TEXT NOT NULL,
    PRIMARY KEY (package_id, version, file_name, symbol_key),
    FOREIGN KEY (package_id, version) REFERENCES symbol_packages(package_id, version) ON DELETE CASCADE
);
-- Not unique: one assembly can ship in several packages, and equal keys mean equal bytes.
CREATE INDEX symbol_files_lookup ON symbol_files (file_name, symbol_key);
