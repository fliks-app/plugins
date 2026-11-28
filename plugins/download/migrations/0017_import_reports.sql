CREATE TABLE IF NOT EXISTS "import_reports" (
  "historyId" integer PRIMARY KEY REFERENCES "download_history"("id") ON DELETE CASCADE,
  "files" jsonb NOT NULL DEFAULT '[]',
  "log" jsonb NOT NULL DEFAULT '[]',
  "only" jsonb,
  "updatedAt" timestamptz NOT NULL DEFAULT now()
);
