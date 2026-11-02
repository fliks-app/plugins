ALTER TABLE "indexers"
  ADD COLUMN IF NOT EXISTS "capsBookSearch" boolean NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS "capsBookSearchParams" text,
  ADD COLUMN IF NOT EXISTS "capsCategories" text
