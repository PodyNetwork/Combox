-- CVs uploaded with jobs-board applications (PDF only, up to 5 MB). Only
-- MFA-verified admins can download them. Deleted with the application, and
-- by the candidate retention purge or a deletion request.
CREATE TABLE axom.board_application_cvs (
    application_id UUID PRIMARY KEY REFERENCES axom.board_applications (id) ON DELETE CASCADE,
    filename       TEXT NOT NULL,
    content        BYTEA NOT NULL,
    size_bytes     INT NOT NULL CHECK (size_bytes > 0 AND size_bytes <= 5242880),
    sha256         TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
