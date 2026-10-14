CREATE TABLE audit_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
    enabled BOOLEAN NOT NULL DEFAULT false,
    last_attempt TIMESTAMPTZ,
    last_success TIMESTAMPTZ,
    last_error TEXT
);
INSERT INTO audit_settings(singleton) VALUES(true);
CREATE TABLE audit_snapshots (
    hash TEXT PRIMARY KEY,
    data JSONB NOT NULL,
    current BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX audit_current_idx ON audit_snapshots(current) WHERE current;
