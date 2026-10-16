ALTER TABLE upstream_cache ADD COLUMN mirrored BOOLEAN NOT NULL DEFAULT false;
CREATE TABLE upstream_mirrors (
    source_id UUID PRIMARY KEY REFERENCES upstream_sources(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT false,
    interval_seconds INTEGER NOT NULL DEFAULT 3600 CHECK(interval_seconds BETWEEN 60 AND 86400),
    requested BOOLEAN NOT NULL DEFAULT false,
    status TEXT NOT NULL DEFAULT 'idle' CHECK(status IN ('idle','queued','running','completed','failed','paused')),
    generation UUID, source_url TEXT,
    cursor JSONB NOT NULL DEFAULT '{}'::jsonb,
    discovery_complete BOOLEAN NOT NULL DEFAULT false,
    started_at TIMESTAMPTZ, completed_at TIMESTAMPTZ,
    next_run TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT
);
CREATE TABLE upstream_mirror_jobs (
    source_id UUID NOT NULL REFERENCES upstream_mirrors(source_id) ON DELETE CASCADE,
    package_id TEXT NOT NULL, version TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL CHECK(action IN ('sync','delete')),
    event_time TIMESTAMPTZ NOT NULL,
    generation UUID NOT NULL,
    done BOOLEAN NOT NULL DEFAULT false,
    skipped BOOLEAN NOT NULL DEFAULT false,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    PRIMARY KEY(source_id,package_id,version)
);
CREATE INDEX upstream_mirror_due_idx ON upstream_mirror_jobs(source_id,next_attempt) WHERE NOT done AND attempts<6;
