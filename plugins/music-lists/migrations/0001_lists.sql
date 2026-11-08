CREATE TABLE lists (
  "id" bigserial PRIMARY KEY,
  "name" text NOT NULL,
  "implementation" text NOT NULL,
  "settings" jsonb NOT NULL DEFAULT '{}',
  "enabled" boolean NOT NULL DEFAULT true,
  "ownerId" integer,
  "lastSyncAt" timestamptz,
  "lastError" text,
  "lastSummary" jsonb,
  "createdAt" timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE list_entries (
  "listId" bigint NOT NULL REFERENCES lists("id") ON DELETE CASCADE,
  "key" text NOT NULL,
  "actions" text NOT NULL,
  "mediaId" integer,
  "itemId" integer,
  "checkedAt" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("listId", "key")
);

CREATE TABLE list_secrets (
  "listId" bigint PRIMARY KEY REFERENCES lists("id") ON DELETE CASCADE,
  "data" jsonb NOT NULL DEFAULT '{}'
);

CREATE TABLE oauth_states (
  "state" text PRIMARY KEY,
  "listId" bigint NOT NULL REFERENCES lists("id") ON DELETE CASCADE,
  "createdAt" timestamptz NOT NULL DEFAULT now()
);
