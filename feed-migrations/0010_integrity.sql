CREATE TABLE integrity_policy (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
    enabled BOOLEAN NOT NULL DEFAULT false,
    interval_hours INTEGER NOT NULL DEFAULT 24 CHECK(interval_hours BETWEEN 1 AND 8760),
    next_scan TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO integrity_policy DEFAULT VALUES;
CREATE TABLE integrity_runs (
    id UUID PRIMARY KEY,
    status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','running','completed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX integrity_active_idx ON integrity_runs((true)) WHERE status IN ('queued','running');
CREATE TABLE integrity_items (
    run_id UUID NOT NULL REFERENCES integrity_runs(id) ON DELETE CASCADE,
    storage_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('package','cache','symbol')),
    package_id TEXT NOT NULL,
    version TEXT NOT NULL,
    expected_size BIGINT NOT NULL,
    expected_digest TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','ok','missing','mismatch','unavailable','obsolete')),
    actual_size BIGINT,
    actual_digest TEXT,
    checked_at TIMESTAMPTZ,
    PRIMARY KEY(run_id,storage_key)
);
CREATE INDEX integrity_pending_idx ON integrity_items(run_id,storage_key) WHERE status='pending';
CREATE INDEX integrity_result_idx ON integrity_items(run_id,status,storage_key);
