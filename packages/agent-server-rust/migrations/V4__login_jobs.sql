CREATE TABLE login_jobs (
    id TEXT PRIMARY KEY,
    phase TEXT NOT NULL,
    error_code TEXT,
    updated_at TEXT NOT NULL
);
