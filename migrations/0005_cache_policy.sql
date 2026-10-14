ALTER TABLE upstream_cache ADD COLUMN last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE upstream_cache ADD COLUMN protected BOOLEAN NOT NULL DEFAULT false;
CREATE INDEX upstream_cache_access_idx ON upstream_cache(last_accessed_at) WHERE NOT protected;
CREATE TABLE cache_policy (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
    enabled BOOLEAN NOT NULL DEFAULT false,
    max_bytes BIGINT NOT NULL DEFAULT 0 CHECK(max_bytes>=0),
    idle_days INTEGER NOT NULL DEFAULT 0 CHECK(idle_days BETWEEN 0 AND 3650),
    orphan_cursor TEXT,
    last_run TIMESTAMPTZ,
    last_result JSONB
);
INSERT INTO cache_policy(singleton) VALUES(true);
CREATE TABLE storage_cleanup (
    storage_key TEXT PRIMARY KEY,
    not_before TIMESTAMPTZ NOT NULL DEFAULT now()+interval '1 hour'
);
