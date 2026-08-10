/**
 * Config arrives as `FLIKS_CFG_<KEY>` env vars, fixed for the life of the
 * process. A `config` note only names which settings keys changed, not
 * their new values (see `PluginApi['config']` in the Fliks contract) — a
 * running process has no way to fetch the new value without a core-side
 * method this phase does not implement, so the note is logged and the
 * effective config only actually changes on the next respawn.
 */
export interface PluginConfig {
  targetUrl: string | null;
}

export function readConfig(env: NodeJS.ProcessEnv = process.env): PluginConfig {
  return { targetUrl: env.FLIKS_CFG_TARGET_URL?.trim() || null };
}
