CREATE TABLE web_sessions ("id" varchar not null primary key, "data" text not null, "expires_at" integer not null);
CREATE INDEX web_sessions_expires_at_index ON web_sessions ("expires_at");

-- Laravel encrypted these with its APP_KEY; users re-enroll two-factor authentication instead.
UPDATE users SET two_factor_secret = NULL, two_factor_recovery_codes = NULL;

-- Replaced by web_sessions, the in-process hashing worker, and the absence of password resets.
DROP TABLE IF EXISTS sessions;
DROP TABLE IF EXISTS jobs;
DROP TABLE IF EXISTS failed_jobs;
DROP TABLE IF EXISTS password_resets;
