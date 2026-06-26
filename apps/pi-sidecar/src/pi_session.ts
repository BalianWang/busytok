/**
 * Represents a single adapter session in the hot session pool.
 * A session is "hot" while it occupies a slot in the pool; closing it
 * frees the slot for reuse.
 */
export interface PiSession {
  adapter_session_id: string;
  logical_subagent_id: string;
  created_at_ms: number;
  last_used_at_ms: number;
}
