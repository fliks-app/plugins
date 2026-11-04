ALTER TABLE "download_history" ADD COLUMN IF NOT EXISTS "retryLaterSince" timestamptz
