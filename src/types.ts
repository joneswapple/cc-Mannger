// Shapes returned by /admin/api/*. Timestamps are unix seconds.

export interface Quota {
  five_hour_utilization: number | null;
  five_hour_reset: number | null;
  seven_day_utilization: number | null;
  seven_day_reset: number | null;
  status: string;
  updated_at: number;
}

export interface Account {
  id: number;
  label: string;
  email: string;
  org_uuid: string;
  plan: string;
  priority: number;
  weight: number;
  enabled: boolean;
  status: string;
  status_reason: string;
  proxy_url: string;
  source_path: string;
  sync_file: boolean;
  cooldown_until: number;
  account_group: string;
  last_used_at: number;
  created_at: number;
  updated_at: number;
  quota?: Quota | null;
}

export interface PoolConfig {
  strategy: string;
  sticky: boolean;
  sticky_ttl_secs: number;
  max_retries: number;
  cooldown_secs: number;
  quota_soft_limit?: number;
}

export interface AccountsResponse {
  accounts: Account[];
  now: number;
  pool: PoolConfig;
  recommended: number | null;
}

export interface Policy {
  allowed_models: string[] | null;
  daily_token_limit: number | null;
  rpm_limit: number | null;
  account_group: string | null;
}

export interface KeyInfo {
  id: number;
  prefix: string;
  created_at: number;
  last_used_at: number | null;
  revoked_at: number | null;
}

export interface User {
  id: number;
  label: string;
  enabled: boolean;
  created_at: number;
  keys: KeyInfo[];
  policy: Policy;
}

export interface Invite {
  token_hash: string;
  label: string;
  created_at: number;
  expires_at: number;
  redeemed_at: number | null;
  user_id: number | null;
}

export interface Status {
  version: string;
  listen: string;
  accounts: number;
  upstream: string;
  catalog: { models: number; last_fetched: number | null; last_error: string | null };
}

export interface Rollup {
  requests: number;
  input_tokens: number;
  output_tokens: number;
}

export interface Stats {
  days: number;
  by_day: (Rollup & { date: string; cache_read_tokens: number; errors: number; avg_latency_ms: number })[];
  by_user: (Rollup & { user_id: number; label: string })[];
  by_model: (Rollup & { model: string })[];
  by_account: (Rollup & { account_id: number; label: string })[];
}

export interface UsageEvent {
  id: number;
  ts: number;
  user_id: number;
  user_label: string;
  key_id: number;
  account_id: number | null;
  model: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  latency_ms: number;
  status: number;
  error: string;
}

export interface UsagePage {
  events: UsageEvent[];
  total: number;
  page: number;
  page_size: number;
}

export interface Model {
  id: string;
  display_name: string;
}

export interface ModelRoute {
  pattern: string;
  target: string;
}

export interface Settings {
  disabled_models: string[];
  models: Model[];
  catalog: Status["catalog"];
  model_routes?: ModelRoute[];
  upstream: {
    source: string;
    base_url: string;
    anthropic_beta: string;
    oauth_token_url: string;
    oauth_client_id: string;
    user_agent: string;
    inject_claude_code_system: boolean;
    effort_levels: Record<string, { max_tokens: number; thinking_budget: number }>;
  };
}
