CREATE TABLE plan_questions (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    turn_id TEXT NOT NULL,
    issuer_agent_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('clarification', 'executionConsent')),
    question TEXT NOT NULL,
    options_json TEXT NOT NULL CHECK (json_valid(options_json)),
    scope_digest TEXT NOT NULL,
    scope_context_json TEXT NOT NULL CHECK (json_valid(scope_context_json)),
    state TEXT NOT NULL CHECK (state IN ('pending', 'answered', 'approved', 'denied', 'expired', 'cancelled')),
    resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
    created_at_ms INTEGER NOT NULL,
    deadline_at_ms INTEGER NOT NULL,
    resolved_at_ms INTEGER,
    CHECK ((state = 'pending' AND resolution_json IS NULL AND resolved_at_ms IS NULL)
        OR (state <> 'pending' AND resolution_json IS NOT NULL AND resolved_at_ms IS NOT NULL))
) STRICT;
CREATE INDEX idx_plan_questions_pending
    ON plan_questions(state, deadline_at_ms, created_at_ms, id);
CREATE INDEX idx_plan_questions_task_turn
    ON plan_questions(task_id, turn_id, created_at_ms, id);

UPDATE schema_version SET version = 21 WHERE singleton_id = 1;
