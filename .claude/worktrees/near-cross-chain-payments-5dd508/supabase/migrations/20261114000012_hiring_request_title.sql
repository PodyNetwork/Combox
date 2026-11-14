-- AI workers can ask HR to hire a freelancer for their task (the
-- request_hire tool). The worker names the role; the job posting uses that
-- title instead of the planned role's or a generic "Freelancer: <skills>".
-- NULL for requests opened by routing.

ALTER TABLE axom.hiring_requests ADD COLUMN title TEXT;
