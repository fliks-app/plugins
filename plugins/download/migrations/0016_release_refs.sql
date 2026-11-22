CREATE TABLE IF NOT EXISTS "release_refs" (
  "ref" varchar PRIMARY KEY,
  "downloadUrl" text NOT NULL,
  "title" text NOT NULL,
  "indexerId" integer NOT NULL REFERENCES "indexers"("id") ON DELETE CASCADE,
  "createdAt" timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS "idx_release_refs_created_at" ON "release_refs" ("createdAt");
CREATE INDEX IF NOT EXISTS "idx_indexer_stats_indexer_date" ON "indexer_stats" ("indexerId", "queryDate");
CREATE INDEX IF NOT EXISTS "idx_download_history_status_updated" ON "download_history" ("status", "updatedAt");
