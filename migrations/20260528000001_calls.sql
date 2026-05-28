-- Voice call log table
CREATE TABLE calls (
    id          TEXT    PRIMARY KEY,
    sim_id      TEXT    NOT NULL,
    phone       TEXT,
    direction   TEXT    NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    status      TEXT    NOT NULL DEFAULT 'ringing',
    started_at  DATETIME DEFAULT CURRENT_TIMESTAMP,
    ended_at    DATETIME
);

CREATE INDEX idx_calls_sim_id ON calls (sim_id);
CREATE INDEX idx_calls_started_at ON calls (started_at DESC);
