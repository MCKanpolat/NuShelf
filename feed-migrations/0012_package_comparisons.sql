CREATE TABLE package_comparisons (
    id UUID PRIMARY KEY,
    package_id TEXT NOT NULL,
    baseline_version TEXT NOT NULL,
    target_version TEXT NOT NULL,
    inputs JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','running','completed','failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    lease_token UUID,
    lease_until TIMESTAMPTZ,
    report JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);
CREATE INDEX package_comparisons_history ON package_comparisons(package_id,created_at DESC);
CREATE INDEX package_comparisons_pending ON package_comparisons(created_at) WHERE status IN ('queued','running');
CREATE UNIQUE INDEX package_comparisons_active ON package_comparisons(package_id,baseline_version,target_version) WHERE status IN ('queued','running');
