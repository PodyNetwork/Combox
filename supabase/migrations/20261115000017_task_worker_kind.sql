-- Who should do a task: an AI agent, a human freelancer, or NULL to let
-- Combox decide (tools mean an AI agent; a planned human role covering the
-- skills means a freelancer). A human is never given a task that needs tools.
ALTER TABLE axom.tasks ADD COLUMN worker_kind TEXT
    CHECK (worker_kind IN ('ai_agent', 'human_freelancer'));
ALTER TABLE axom.tasks ADD CONSTRAINT tasks_human_no_tools
    CHECK (worker_kind IS DISTINCT FROM 'human_freelancer' OR cardinality(required_tools) = 0);
