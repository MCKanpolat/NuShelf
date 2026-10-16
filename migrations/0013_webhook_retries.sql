ALTER TABLE webhooks ADD COLUMN revision BIGINT NOT NULL DEFAULT 1;
ALTER TABLE webhook_deliveries ADD COLUMN attempts_in_cycle INTEGER NOT NULL DEFAULT 0;
ALTER TABLE webhook_deliveries ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
UPDATE webhook_deliveries SET attempts_in_cycle = LEAST(attempts, 6);

CREATE TABLE webhook_delivery_attempts (
    delivery_id UUID NOT NULL REFERENCES webhook_deliveries(id),
    attempt_number INTEGER NOT NULL,
    cycle INTEGER NOT NULL,
    webhook_revision BIGINT NOT NULL,
    receiver_url TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    status INTEGER,
    error TEXT,
    PRIMARY KEY (delivery_id, attempt_number)
);
