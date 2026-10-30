/**
 * Vunly TypeScript SDK (S1: customers, usage, credits). `fetch` only, zero dependencies.
 * The caller supplies the key (read it from the environment); this package reads nothing from disk and never
 * prints, logs or embeds the key in an error.
 */

import { VERSION } from "./version.ts";

export * from "./webhooks.ts";

export interface ClientOptions {
  /** Sandbox key (`vk_test_...`). Read it from the environment, never hard-code it. */
  apiKey: string;
  /** The API address, e.g. `http://127.0.0.1:8787`. A trailing slash or `/v1` is accepted and ignored. */
  baseUrl: string;
  /** Override for tests or non-standard runtimes. */
  fetch?: typeof fetch;
  /**
   * Retries after a network failure, a timeout, 429 (after `Retry-After`) or 500/502/503/504, always with the SAME
   * id. Default 2; 0 turns retries off. 400/401/402/403/404/409 are never retried.
   */
  maxRetries?: number;
  /** Per-attempt timeout in milliseconds. Default 10000. */
  timeoutMs?: number;
  /** Replaces the wait between retries (tests). */
  sleep?: (ms: number) => Promise<void>;
}

const RETRYABLE = new Set([0, 429, 500, 502, 503, 504]);
/** R reads and the entitlement check; K keyed mutations; N naturally idempotent writes (upserts, deletes); X the rest. */
type OperationClass = "R" | "K" | "N" | "X";
const USER_AGENT = `vunly-js/${VERSION} (${typeof process !== "undefined" && process.versions?.node ? `node ${process.versions.node}` : "web"})`;
const MAX_RETRY_AFTER_SECONDS = 30;
/** The largest wait reported to the caller; the SDK itself never waits more than `MAX_RETRY_AFTER_SECONDS`. */
const MAX_REPORTED_RETRY_AFTER_SECONDS = 86_400;
/**
 * The whole syntax of a Retry-After this SDK honours (conformance/README.md): ASCII digits, an optional
 * fraction, an optional exponent, an optional plus, or Inf / Infinity. `Number()` alone would also accept "", "0x10", " 5 " and
 * "1e999" (as Infinity, which `Number.isFinite` then dropped, so the 429 was retried early).
 */
const RETRY_AFTER =
  /^[ \t]*(\+?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?|\+?inf(?:inity)?)[ \t]*$/i;

/**
 * The wait a Retry-After asks for in seconds (capped at one day), or undefined when it is not a number (an HTTP date, garbage: the
 * normal backoff applies). A number too large to represent (`1e999`, hundreds of digits, `Inf`) is a very long wait, NOT an absent
 * header: it is capped, and the call surfaces it at once instead of retrying early.
 */
function parseRetryAfter(header: string | null | undefined): number | undefined {
  if (header === null || header === undefined) return undefined;
  const m = RETRY_AFTER.exec(header);
  if (!m) return undefined;
  const text = m[1]!;
  const secs = /inf/i.test(text) ? Infinity : Number(text);
  return Math.min(secs, MAX_REPORTED_RETRY_AFTER_SECONDS);
}

/** Seconds to wait before retry number `attempt` (1 = first), or null to give up (a Retry-After longer than the cap). */
function retryDelay(attempt: number, retryAfter: number | undefined): number | null {
  // 0 is "no directive": the normal backoff applies
  if (retryAfter !== undefined && retryAfter > 0)
    return retryAfter <= MAX_RETRY_AFTER_SECONDS ? retryAfter : null;
  return Math.min(8, 0.25 * 2 ** (attempt - 1)) * (0.5 + Math.random() / 2);
}

export interface Customer {
  /** Vunly's own id (`cus_<hex>`). You never have to store it: every call accepts your `externalId` instead. */
  id: string;
  /** The id you passed to `customers.upsert`. Use it as `customer` in `check`, `record`, `balance` and `usage`. */
  externalId: string;
  name: string | null;
  createdAt: string;
}

export interface LedgerEntry {
  id: string;
  customerId: string;
  kind: "grant" | "debit";
  credits: number;
  meter: string | null;
  quantity: number | null;
  reason: string | null;
  idempotencyKey: string;
  createdAt: string;
}

export interface UsageResult {
  event: LedgerEntry;
  /** Credits left after this event. */
  remaining: number;
  /** True when this `id` had already been recorded: nothing was debited again. */
  replayed: boolean;
  /**
   * True when the project is on a paid plan and this event is past the plan's included monthly volume. Paid plans keep
   * recording; this lets you notice. Always false for a replay.
   */
  planOverage: boolean;
}

export interface Balance {
  remaining: number;
  consumed: number;
  granted: number;
}

export type ErrorCode =
  | "validation_error"
  | "unauthenticated"
  | "key_revoked"
  | "forbidden"
  | "missing_scope"
  | "production_not_enabled"
  | "not_found"
  | "meter_not_found"
  | "idempotency_conflict"
  | "subscription_exists"
  | "insufficient_credits"
  | "rate_limited"
  | "internal_error"
  | "unavailable"
  /** Raised by the SDK itself (never sent by the API): a 2xx response lacked a promised field. */
  | "invalid_response"
  /** Raised by the SDK itself: the API answered a 3xx. Never followed (the key would go with it) and never retried. */
  | "unexpected_redirect";

/** Base class. `code` is stable; `requestId` helps support. The message never contains your key. */
export class VunlyError extends Error {
  readonly code: ErrorCode;
  readonly status: number;
  readonly requestId: string | undefined;
  readonly details: Record<string, unknown> | undefined;
  constructor(
    code: ErrorCode,
    message: string,
    status: number,
    requestId?: string,
    details?: Record<string, unknown>,
  ) {
    super(message);
    this.name = new.target.name;
    this.code = code;
    this.status = status;
    this.requestId = requestId;
    this.details = details;
  }
}
/** 401/403: missing, wrong, revoked key, or the key lacks a scope. Not retryable. */
export class AuthError extends VunlyError {}
/** 402: the customer has too few credits. Do not run the work. `details` has `balance` and `required`. */
export class LimitReached extends VunlyError {}
/** 400/404/409: fix the request (bad fields, unknown customer, idempotency id reused with different data). */
export class InvalidRequest extends VunlyError {}
/**
 * Network failure, timeout, 429 or 5xx that survived the SDK's own retries. Safe to retry later with the SAME id.
 * `retryAfterSeconds` is the server's Retry-After when it sent one.
 */
export class Unavailable extends VunlyError {
  retryAfterSeconds: number | undefined;
}

/** The code the API would send for a status when the error body is missing. */
function fallbackCode(status: number): ErrorCode {
  if (status === 401 || status === 403) return "unauthenticated";
  if (status === 402) return "insufficient_credits";
  if (status === 429) return "rate_limited";
  if (status >= 500) return "internal_error";
  return "validation_error";
}

const MAX_MESSAGE_CHARS = 500;
const MAX_CODE_CHARS = 64;
const MAX_REQUEST_ID_CHARS = 128;
/** Containers nested deeper than this (details itself is depth 0) become null; at most this many entries per object or array. */
const MAX_DETAILS_DEPTH = 4;
const MAX_DETAILS_ENTRIES = 50;
/** Unicode categories Cc (controls), Cf (format: BiDi, zero-width, tags, ...), Zl and Zp: the same set as the Go and Java SDKs. */
const INVISIBLE = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu;
function stripInvisible(text: string): string {
  return text.replace(INVISIBLE, "");
}

/**
 * Text from the server that people read (logs, terminals): invisible and control characters out FIRST (so a key with a
 * zero-width character inside is whole again), then every occurrence of the key replaced, and only then bounded (so a cut can
 * never leave half a key behind).
 */
function scrub(text: string, max: number, key: string): string {
  let s = stripInvisible(text);
  if (key) s = s.split(key).join("<hidden>");
  const chars = Array.from(s.trim());
  return chars.length > max ? chars.slice(0, max).join("") : chars.join("");
}

function scrubValue(v: unknown, key: string, depth = 0): unknown {
  if (typeof v === "string") return scrub(v, MAX_MESSAGE_CHARS, key);
  if (v === null || typeof v !== "object") return v;
  if (depth > MAX_DETAILS_DEPTH) return null;
  if (Array.isArray(v))
    return v.slice(0, MAX_DETAILS_ENTRIES).map((e) => scrubValue(e, key, depth + 1));
  const out: Record<string, unknown> = {};
  let n = 0;
  for (const [k, e] of Object.entries(v)) {
    if (n >= MAX_DETAILS_ENTRIES) break;
    const name = scrub(k, MAX_CODE_CHARS, key);
    if (name === "" || name === "__proto__") continue;
    n++;
    out[name] = scrubValue(e, key, depth + 1);
  }
  return out;
}

const scrubId = (v: unknown, key: string): string | undefined =>
  typeof v === "string" ? scrub(v, MAX_REQUEST_ID_CHARS, key) : undefined;

/**
 * A 3xx, the browser's opaque redirect (type "opaqueredirect", status 0), or an answer a caller-supplied fetch already followed
 * (`redirected`): this is the SDK's own error, whatever the body.
 */
function isRedirect(res: Response): boolean {
  return (
    res.type === "opaqueredirect" ||
    res.redirected === true ||
    (res.status >= 300 && res.status < 400)
  );
}

function redirectError(res: Response, key: string): Unavailable {
  void res.body?.cancel().catch(() => {});
  return new Unavailable(
    "unexpected_redirect",
    `the API answered a redirect${res.status >= 300 && res.status < 400 ? ` (${res.status})` : ""}, which is never followed (check the API address you configured)`,
    res.status,
    scrubId(res.headers.get("request-id") ?? undefined, key),
  );
}

function classify(status: number, code: ErrorCode): typeof VunlyError {
  if (status === 402) return LimitReached;
  if (status === 401 || status === 403) return AuthError;
  if (status === 429 || status >= 500) return Unavailable;
  return InvalidRequest;
}

export type CheckReason = "ok" | "no_active_plan" | "feature_not_in_plan" | "insufficient_credits";

export interface Entitlement {
  allowed: boolean;
  /** Typed reason. Anything but `ok` means: do not run the work. */
  reason: CheckReason;
  /** Credits available now. */
  remaining: number;
  /** Credits the estimate would cost. */
  required: number;
  plan: string | null;
}

export interface Plan {
  key: string;
  name: string;
  includedCredits: number;
  meter: string;
  features: string[];
}

export interface RecipeParams {
  includedCredits: number;
  unitsPerCredit: number;
  unitAmount?: number;
}

/** A versioned billing configuration template. `fixture` holds sample usage and the exact ledger values it yields. */
export interface Recipe {
  id: string;
  version: number;
  name: string;
  summary: string;
  semantics: string;
  parameters: RecipeParams;
  meter: string;
  plan: string;
  features: string[];
  price: { currency: string; unitAmount: number; interval: "month" | "year" } | null;
  fixture: {
    customer: string;
    topUp?: number;
    startBalance: number;
    steps: {
      id: string;
      quantity: number;
      expect: { credits: number; balance: number } | { error: "insufficient_credits" };
    }[];
  };
}

export interface AppliedRecipe {
  recipe: { id: string; version: number };
  meter: { key: string; unitsPerCredit: number };
  plan: Plan;
  created: { meter: boolean; plan: boolean };
}

export interface WebhookEndpoint {
  id: string;
  url: string;
  description: string | null;
  /** `["*"]` (everything) or a list of event types. */
  events: string[];
  status: "enabled" | "disabled";
  /** The signing secret is never shown again after creation; this helps you tell secrets apart. */
  secretLast4: string;
  createdAt: string;
  updatedAt: string;
}

export interface WebhookDelivery {
  id: string;
  endpointId: string;
  /** The event's id, the same on every attempt: dedupe on it. */
  eventId: string;
  eventType: string;
  status: "pending" | "sent" | "failed";
  attempts: number;
  lastStatus: number | null;
  lastError: string | null;
  nextAttemptAt: string | null;
  deliveredAt: string | null;
  createdAt: string;
}

export interface WebhookDeliveryDetail extends WebhookDelivery {
  event: { id: string; type: string; createdAt: string; data: unknown };
  attemptLog: {
    n: number;
    at: string;
    statusCode: number | null;
    error: string | null;
    durationMs: number | null;
  }[];
}

export interface ActivityItem {
  id: string;
  at: string;
  /** The `requestId` of an error (`VunlyError.requestId`) or the `Request-Id` response header. */
  requestId: string;
  type: string;
  method: string;
  route: string;
  status: number;
  errorCode: string | null;
  summary: string;
  customerRef: string | null;
  actor: { kind: "key" | "session"; id: string | null };
  durationMs: number | null;
}

export interface Subscription {
  id: string;
  customerId: string;
  plan: Plan;
  status: "active" | "canceled";
  periodStart: string;
  periodEnd: string;
  /** The change waiting for the next renewal. Present on `subscribe` results of a change and on reads. */
  pendingChange?: SubscriptionChange | null;
}

export interface BillingDiagnostics {
  connection: { provider: string; status: "active" | "revoked"; connectedAt: string } | null;
  outbox: {
    pending: number;
    sent: number;
    failed: number;
    retrying: number;
    oldestPendingAt: string | null;
  };
  webhooks: { processed: number; failed: number; lastReceivedAt: string | null };
  reconciliation: {
    open: number;
    resolved: number;
    lastRun: {
      at: string;
      windowStart: string;
      windowEnd: string;
      checked: number;
      mismatches: number;
      resolved: number;
      failedOutbox: number;
    } | null;
  };
}

export interface ReconciliationItem {
  id: string;
  kind: "usage_mismatch" | "outbox_failed";
  customerId: string | null;
  customerExternalId: string | null;
  windowStart: string;
  windowEnd: string;
  /** Credits the ledger consumed in the window. */
  localValue: number | null;
  /** What the payment provider reports for the same window. */
  remoteValue: number | null;
  status: "open" | "resolved";
  detail: string | null;
  createdAt: string;
  resolvedAt: string | null;
}

/** What the usage widget renders: one customer's balance, plan and usage this period. */
export interface CustomerUsage {
  customer: { id: string; externalId?: string };
  plan: { key: string; name: string } | null;
  period: { start: string; end: string } | null;
  periodEnd: string | null;
  balance: { credits: number; granted: number; consumed: number };
  meters: { meter: string; quantity: number; credits: number; events: number }[];
  /** Whether the widget shows the vendor mark: decided by the project's contracted plan, not by you. */
  brand: boolean;
}

/**
 * Read one customer's usage with a customer token (`customerTokens.create`). Safe in a browser: the token reads this
 * customer's balance and nothing else, and expires. Throws `AuthError` when the token is invalid or expired (ask
 * your server for a new one).
 */
export async function getCustomerUsage(options: {
  baseUrl: string;
  token: string;
  fetch?: typeof fetch;
  /** Give up after this long (default 10 s), so a server that accepts and never answers does not hang the caller. */
  timeoutMs?: number;
}): Promise<CustomerUsage> {
  const doFetch = options.fetch ?? fetch;
  let res: Response;
  try {
    res = await doFetch(`${normalizeBaseUrl(options.baseUrl)}/v1/customer/usage`, {
      headers: { Authorization: `Bearer ${options.token}` },
      redirect: "manual",
      signal: AbortSignal.timeout(options.timeoutMs ?? 10_000),
    });
  } catch {
    // a network failure or the timeout: deliberately not the underlying error (it may carry the request headers)
    throw new Unavailable("unavailable", "could not reach the Vunly API", 0);
  }
  if (isRedirect(res)) throw redirectError(res, options.token);
  const body = (await res.json().catch(() => null)) as
    (CustomerUsage & { error?: { code?: ErrorCode; message?: string; requestId?: string } }) | null;
  if (!res.ok) {
    const e = body?.error;
    const auth = res.status === 401 || res.status === 403;
    const rawCode = typeof e?.code === "string" ? scrub(e.code, MAX_CODE_CHARS, options.token) : "";
    const code = (rawCode || fallbackCode(res.status)) as ErrorCode;
    const msg =
      typeof e?.message === "string" ? scrub(e.message, MAX_MESSAGE_CHARS, options.token) : "";
    throw new (classify(res.status, code))(
      code,
      msg || (auth ? "customer token rejected" : `request failed with status ${res.status}`),
      res.status,
      scrubId(e?.requestId, options.token),
    );
  }
  const problem = shapeProblem(body, { "customer.id": "string", ...USAGE_SHAPE });
  if (problem)
    throw new Unavailable(
      "invalid_response",
      `the API answered ${res.status} but the response is malformed (${problem})`,
      res.status,
      scrubId(res.headers.get("request-id") ?? undefined, options.token),
    );
  return body as CustomerUsage;
}

export interface ExportOptions {
  /** Ledger and usage only: the Vunly id or your own id. */
  customer?: string;
  /** ISO times, ledger and usage only. */
  from?: string;
  to?: string;
  limit?: number;
  cursor?: string;
}

export interface CsvExport {
  /** RFC 4180 text with a header row. */
  csv: string;
  /** Pass as `cursor` to continue; null when this was the last page. */
  nextCursor: string | null;
}

export interface CreditPack {
  key: string;
  name: string;
  credits: number;
  /** What the provider charges once (smallest currency unit), or null for a pack not sold through a provider. */
  price: { currency: string; unitAmount: number } | null;
  updatedAt: string;
}

export interface SubscriptionChange {
  id: string;
  subscriptionId: string;
  from: string;
  to: string;
  kind: "upgrade" | "downgrade" | "lateral";
  timing: "now" | "period_end";
  status: "applied" | "pending" | "canceled";
  /** An immediate upgrade grants the difference in included credits. */
  creditsGranted: number;
  effectiveAt: string;
  createdAt: string;
}

export interface UsageSummary {
  customerId: string;
  plan: { key: string; name: string } | null;
  period: { start: string; end: string } | null;
  balance: { credits: number; granted: number; consumed: number };
  meters: { meter: string; quantity: number; credits: number; events: number }[];
}

/** One page of a list. `nextCursor` is opaque; pass it back as `cursor` for the next page, `null` means the end. */
export interface Page<T> {
  data: T[];
  nextCursor: string | null;
}

/**
 * The project's plan and this month's recorded usage (`planUsage`). The included volume and retention are a working
 * hypothesis, not approved and not a guarantee: they stay inside `unconfirmedLimits` with `confirmed: false`. Do not show
 * them to your customers as a commitment.
 */
export interface PlanUsage {
  plan: { key: string; name: string };
  period: { start: string; end: string };
  usage: { events: number };
  unconfirmedLimits: {
    confirmed: boolean;
    note: string;
    includedMonthlyEvents: number | null;
    activityRetentionDays: number | null;
  };
}

export interface Client {
  customers: {
    /** Idempotent: creates the customer or returns the existing one for this id. */
    upsert(input: { id: string; name?: string }): Promise<Customer>;
    /** One customer, by your id or Vunly's. `InvalidRequest` (`not_found`) when it does not exist. */
    get(input: { customer: string }): Promise<Customer>;
    /** Newest first, `limit` 1 to 100 (default 25). Follow `nextCursor` until it is `null`. */
    list(input?: { limit?: number; cursor?: string }): Promise<Page<Customer>>;
  };
  /** The project's plan and this month's usage. */
  planUsage(): Promise<PlanUsage>;
  entitlements: {
    /**
     * Read-only decision before you do the work. Fail closed: on `Unavailable` do not run the work.
     * `estimate` is what you expect to consume (same meter and unit you will record afterwards). An unknown meter
     * in `estimate` throws `InvalidRequest` (`meter_not_found`), never an "allowed" answer.
     */
    check(input: {
      customer: string;
      feature: string;
      estimate?: { meter: string; quantity: number };
    }): Promise<Entitlement>;
  };
  /** Project setup (the wizard or the verifier does this; the key needs plans:write / subscriptions:write). */
  plans: {
    upsertMeter(input: { key: string; unitsPerCredit: number }): Promise<void>;
    upsert(input: {
      key: string;
      name: string;
      includedCredits: number;
      meter: string;
      features: string[];
    }): Promise<Plan>;
    /** Idempotent: grants the plan's included credits once per period. */
    subscribe(input: { customer: string; plan: string }): Promise<Subscription>;
    cancel(input: { customer: string }): Promise<void>;
    /**
     * Upgrade or downgrade the active subscription. `id` is the Idempotency-Key (8-128 chars): reuse it on retry.
     * Upgrades default to `timing: "now"` and grant the difference in included credits once; downgrades default
     * to `"period_end"` (nothing changes until renewal). Credits are never clawed back. `change` is null when the
     * customer is already on `plan` (any waiting change is dropped). A concurrent change throws `InvalidRequest`
     * with code `subscription_exists`: re-read and retry.
     */
    change(input: {
      id: string;
      customer: string;
      plan: string;
      timing?: "now" | "period_end";
    }): Promise<{
      change: SubscriptionChange | null;
      subscription: Subscription;
      replayed: boolean;
    }>;
    /** Drop the change waiting for renewal. */
    cancelPendingChange(input: { customer: string }): Promise<void>;
  };
  /**
   * CSV exports of this project's data (ledger and usage need ledger:read, customers customers:read). Each call returns
   * at most `limit` rows (default 10,000, max 100,000); when `nextCursor` is set, pass it as `cursor` for the next page.
   * Cells that look like spreadsheet formulas are neutralised.
   */
  exports: {
    ledger(input?: ExportOptions): Promise<CsvExport>;
    usage(input?: ExportOptions): Promise<CsvExport>;
    customers(input?: Pick<ExportOptions, "limit" | "cursor">): Promise<CsvExport>;
  };
  /**
   * Customer-scoped read tokens for embedded widgets (the key needs balance:read). Call `create` from your SERVER;
   * give the token to the browser, which reads it with `getCustomerUsage`. Never put an API key in a browser.
   */
  customerTokens: {
    /** `customer` is the Vunly id or your own id. `ttlSeconds` is 30 to 3600 (default 900). */
    create(input: {
      customer: string;
      ttlSeconds?: number;
    }): Promise<{ id: string; token: string; customerId: string; expiresAt: string }>;
    revoke(input: { id: string }): Promise<void>;
  };
  /** Payment-provider health and reconciliation (the key needs billing:read to look, billing:write to act). */
  billing: {
    diagnostics(): Promise<BillingDiagnostics>;
    reconciliation: {
      list(input?: {
        status?: "open" | "resolved";
        kind?: "usage_mismatch" | "outbox_failed";
        customer?: string;
        limit?: number;
        cursor?: string;
      }): Promise<{ data: ReconciliationItem[]; nextCursor: string | null }>;
      /** Deliver due usage and reconcile the last 24 hours now. */
      run(): Promise<Record<string, unknown>>;
      /**
       * Fix a mismatch by re-sending its usage and re-checking. The provider dedupes on the ledger entry id, so this
       * cannot double count. Throws `InvalidRequest` when the provider has MORE than the ledger (needs a manual review).
       */
      resend(input: { id: string }): Promise<{ item: ReconciliationItem; requeued: number }>;
    };
  };
  /** Credit packs: one-time top-ups. The catalog needs plans:read / plans:write, a purchase credits:grant. */
  creditPacks: {
    upsert(input: {
      key: string;
      name: string;
      credits: number;
      /** Omit to keep the price, null to remove it. */
      price?: { currency: string; unitAmount: number } | null;
    }): Promise<CreditPack>;
    list(): Promise<CreditPack[]>;
    /**
     * Records a purchase you have been paid for (or a sandbox top-up): grants the pack's credits once. `id` is the
     * Idempotency-Key (8-128 chars): reuse it on retry and the credits are granted once.
     */
    purchase(input: {
      id: string;
      customer: string;
      pack: string;
    }): Promise<{ entry: LedgerEntry; balance: number; replayed: boolean }>;
  };
  /** Billing recipes: versioned meter+plan templates (the key needs plans:read / plans:write). */
  recipes: {
    list(): Promise<Recipe[]>;
    /**
     * Creates or updates the recipe's meter and plan in this project. Idempotent; `params` overrides the defaults
     * (`unitAmount` only for priced recipes). Subscribe customers afterwards with `plans.subscribe`.
     */
    apply(input: { recipe: string; params?: Partial<RecipeParams> }): Promise<AppliedRecipe>;
  };
  /**
   * Outbound webhooks (the key needs webhooks:read / webhooks:write). To RECEIVE them use `verifyWebhook`, which
   * is exported separately and needs no client.
   */
  webhooks: {
    endpoints: {
      /** The returned `secret` signs every delivery. It is shown here once; store it in your environment. */
      create(input: {
        url: string;
        events?: string[];
        description?: string;
      }): Promise<WebhookEndpoint & { secret: string }>;
      list(): Promise<WebhookEndpoint[]>;
      update(input: {
        id: string;
        url?: string;
        events?: string[];
        description?: string | null;
        status?: "enabled" | "disabled";
      }): Promise<WebhookEndpoint>;
      delete(input: { id: string }): Promise<void>;
      /** The old secret stops signing at once; the new one is returned once. */
      rotateSecret(input: { id: string }): Promise<WebhookEndpoint & { secret: string }>;
      /** Queues a `webhook.test` event for this endpoint; it is sent on the next delivery pass. */
      test(input: { id: string }): Promise<WebhookDelivery>;
    };
    deliveries: {
      list(input?: {
        endpointId?: string;
        status?: "pending" | "sent" | "failed";
        limit?: number;
        cursor?: string;
      }): Promise<{ data: WebhookDelivery[]; nextCursor: string | null }>;
      /** One delivery with its event payload and every attempt (status code, error, duration). */
      get(input: { id: string }): Promise<WebhookDeliveryDetail>;
      /** Queues a failed or sent delivery again with a fresh attempt budget. Same event id. */
      redeliver(input: { id: string }): Promise<WebhookDeliveryDetail>;
    };
  };
  /** The project's activity log: API errors and state changes (the key needs activity:read). */
  activity: {
    /**
     * Newest first, inside the retention window (`retentionDays`). Pass the `requestId` from a `VunlyError` to find
     * what happened. Successful usage, checks and reads are not logged; use `credits.usage` or the ledger for those.
     */
    list(input?: {
      requestId?: string;
      customer?: string;
      type?: string;
      outcome?: "error" | "ok";
      errorCode?: string;
      from?: string;
      to?: string;
      limit?: number;
      cursor?: string;
    }): Promise<{ data: ActivityItem[]; nextCursor: string | null; retentionDays: number }>;
  };
  usage: {
    /**
     * Records usage once and debits credits. `quantity` is in the meter's own units (for example tokens), not
     * credits: the project's meter converts it, rounded up per request (1050 tokens at 1000 per credit = 2 credits).
     * A meter the project has not configured throws `InvalidRequest` with code `meter_not_found` (status 404) and
     * bills nothing: create it first with `plans.upsertMeter` or a recipe.
     *
     * Idempotent by `id`: use a stable id derived from the unit of work (such as your request id) and reuse it
     * on retry. `id` must be 8 to 128 characters of `A-Za-z0-9_.:-`, otherwise this throws `InvalidRequest`
     * before anything is sent. The same `id` with a different quantity or customer is `InvalidRequest`
     * (`idempotency_conflict`). `LimitReached` is final: the credits were not there when you recorded, even if
     * `entitlements.check` said yes earlier; handle it after the work ran.
     */
    record(input: {
      id: string;
      customer: string;
      meter: string;
      quantity: number;
    }): Promise<UsageResult>;
  };
  credits: {
    balance(input: { customer: string }): Promise<Balance>;
    /** The customer's ledger entries, newest first, `limit` 1 to 100. Follow `nextCursor` until it is `null`. */
    ledger(input: {
      customer: string;
      limit?: number;
      cursor?: string;
    }): Promise<Page<LedgerEntry>>;
    /** Current-period consumption per meter, with plan and balance (customer usage view). */
    usage(input: { customer: string }): Promise<UsageSummary>;
    /** Sandbox top-up. Idempotent by `id`. */
    grant(input: {
      id: string;
      customer: string;
      credits: number;
      reason?: string;
    }): Promise<LedgerEntry>;
  };
}

/** The most of one response the SDK will hold in memory (as the Go and .NET SDKs): the API's largest page is far below it. */
const MAX_RESPONSE_BYTES = 16 * 1024 * 1024;

/**
 * The body as text, or `null` when it is larger than `max` bytes. Read as a stream and abandoned at the cap, so a server that
 * never stops sending costs at most the cap plus one chunk. A body that breaks mid-way rejects, as `Response.text()` does.
 */
async function readCapped(res: Response, max: number): Promise<string | null> {
  if (!res.body) return res.text();
  const reader = res.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      await reader.cancel().catch(() => undefined);
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  return new TextDecoder().decode(all);
}

/** Fields a successful response must carry: dotted path -> typeof. */
type Shape = Record<string, "string" | "number" | "boolean" | "array" | "object">;

function shapeProblem(body: unknown, shape: Shape): string | null {
  if (typeof body !== "object" || body === null || Array.isArray(body))
    return "the body is not a JSON object";
  for (const [path, type] of Object.entries(shape)) {
    let v: unknown = body;
    for (const part of path.split("."))
      v = typeof v === "object" && v !== null ? (v as Record<string, unknown>)[part] : undefined;
    const good =
      type === "array"
        ? Array.isArray(v)
        : type === "object"
          ? typeof v === "object" && v !== null && !Array.isArray(v)
          : typeof v === type;
    if (!good)
      return `field "${path}" is ${v === undefined ? "missing" : `not a${type === "array" || type === "object" ? "n" : ""} ${type}`}`;
  }
  return null;
}

/** The body of a customer's usage view (GET /v1/customers/{id}/usage and GET /v1/customer/usage). */
const USAGE_SHAPE: Shape = {
  "balance.credits": "number",
  "balance.granted": "number",
  "balance.consumed": "number",
  meters: "array",
};

const ENTRY_SHAPE = {
  id: "string",
  customerId: "string",
  kind: "string",
  credits: "number",
  createdAt: "string",
} as const;
const prefixed = (p: string, s: Shape): Shape =>
  Object.fromEntries(Object.entries(s).map(([k, v]) => [`${p}${k}`, v]));

// Shapes for the other modelled bodies, so every method validates its 2xx at one point (`call`): a malformed body
// (missing field, wrong type, null, non-JSON, empty, 204) becomes one Unavailable `invalid_response`, never an
// `undefined` field or a raw TypeError. A list body must be `{ data: [...] }`, not `{}` or `[]`.
const LIST_SHAPE: Shape = { data: "array" };
const PLAN_SHAPE: Shape = {
  key: "string",
  name: "string",
  includedCredits: "number",
  meter: "string",
  features: "array",
};
const SUBSCRIPTION_SHAPE: Shape = {
  id: "string",
  customerId: "string",
  status: "string",
  "plan.key": "string",
};
const ENDPOINT_SHAPE: Shape = { id: "string", url: "string", events: "array", status: "string" };
const DELIVERY_SHAPE: Shape = {
  id: "string",
  endpointId: "string",
  eventId: "string",
  status: "string",
};
const PACK_SHAPE: Shape = { key: "string", name: "string", credits: "number" };

/** The API's Idempotency-Key rule: 8 to 128 characters of A-Za-z0-9_.:- */
const IDEMPOTENCY_ID = /^[A-Za-z0-9_.:-]{8,128}$/;

/**
 * The API address without a trailing slash and without a trailing `/v1`: the SDK adds `/v1/...` itself, so
 * `https://host`, `https://host/`, `https://host/v1` and `https://host/v1/` all mean the same thing.
 */
export function normalizeBaseUrl(url: string): string {
  return url.trim().replace(/\/+$/, "").replace(/\/v1$/i, "").replace(/\/+$/, "");
}

const WRONG_ROUTE_HINT =
  " (check the API address you configured, VUNLY_API_URL or baseUrl: it should be the address only, for example https://host; a trailing /v1 is accepted)";

export function createClient(options: ClientOptions): Client {
  if (!options.apiKey) throw new Error("Vunly: apiKey is empty (read it from your environment)");
  const base = normalizeBaseUrl(options.baseUrl);
  const doFetch = options.fetch ?? fetch;

  type Called<T> = { status: number; replayed: boolean; data: T; text: string; headers: Headers };
  const maxRetries = options.maxRetries ?? 2;
  const timeoutMs = options.timeoutMs ?? 10_000;
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));

  /**
   * One request with the retry rules of conformance/README.md: only Unavailable, only with the same
   * id, and only as the operation class allows. R (reads), K (keyed mutations) and N (naturally idempotent writes) are
   * retried after a lost answer, a timeout or a 5xx; X (everything else) only after a 429, which is refused before it
   * runs: a second endpoint, token or checkout is a different outcome, so after an ambiguous failure the caller decides.
   * A plan-volume stop (429, reason plan_included_volume) is not a rate limit and is never waited on.
   */
  async function call<T>(
    method: string,
    path: string,
    body?: unknown,
    idempotencyKey?: string,
    shape?: Shape,
    cls?: OperationClass,
  ): Promise<Called<T>> {
    const klass: OperationClass =
      cls ??
      (method === "GET"
        ? "R"
        : idempotencyKey !== undefined
          ? "K"
          : method === "DELETE" || method === "PUT" || method === "PATCH"
            ? "N"
            : "X");
    for (let attempt = 1; ; attempt++) {
      try {
        return await once<T>(method, path, body, idempotencyKey, shape);
      } catch (e) {
        // A redirect is never retried, whatever its status (an opaque redirect has status 0, which is otherwise retryable).
        if (e instanceof Unavailable && e.code === "unexpected_redirect") throw e;
        if (!(e instanceof Unavailable) || !RETRYABLE.has(e.status) || attempt > maxRetries)
          throw e;
        if (e.details?.["reason"] === "plan_included_volume") throw e;
        if (klass === "X" && e.status !== 429) throw e;
        const wait = retryDelay(attempt, e.retryAfterSeconds);
        if (wait === null) throw e;
        await sleep(wait * 1000);
      }
    }
  }

  async function once<T>(
    method: string,
    path: string,
    body?: unknown,
    idempotencyKey?: string,
    shape?: Shape,
  ): Promise<Called<T>> {
    // An empty, "." or ".." segment (an id that was empty or dot-only) would change the route on the wire
    // (GET /v1/customers/ , GET /v1/): refused before any request.
    if (
      path
        .split("?")[0]!
        .split("/")
        .slice(1)
        .some((s) => s === "" || s === "." || s === "..")
    ) {
      throw new InvalidRequest(
        "validation_error",
        'an id in the path must not be empty, "." or ".."',
        400,
        undefined,
        { fields: { id: 'not empty, "." or ".."' } },
      );
    }
    if (idempotencyKey !== undefined && !IDEMPOTENCY_ID.test(idempotencyKey)) {
      // Same shape the API would answer with, but without a round trip (and without running anything unbilled).
      throw new InvalidRequest(
        "validation_error",
        "id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)",
        400,
        undefined,
        { fields: { id: "8-128 chars of A-Za-z0-9_.:-" } },
      );
    }
    let res: Response;
    try {
      res = await doFetch(`${base}${path}`, {
        method,
        headers: {
          Authorization: `Bearer ${options.apiKey}`,
          "User-Agent": USER_AGENT,
          ...(body === undefined ? {} : { "Content-Type": "application/json" }),
          ...(idempotencyKey ? { "Idempotency-Key": idempotencyKey } : {}),
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        redirect: "manual",
        signal: AbortSignal.timeout(timeoutMs),
      });
    } catch {
      // Deliberately drop the underlying error: some runtimes include request headers in it.
      throw new Unavailable("unavailable", "could not reach the Vunly API", 0);
    }
    if (isRedirect(res)) throw redirectError(res, options.apiKey);
    let text: string;
    try {
      const read = await readCapped(res, MAX_RESPONSE_BYTES);
      if (read === null) {
        // More than the cap: a broken or hostile server. Never held in memory. A 2xx is the SDK's own error and is not retried
        // (the status is not retryable; the call may have been applied, so a later retry with the SAME id is safe); an error status
        // is classified by its status, as an error with no body is.
        if (res.ok)
          throw new Unavailable(
            "invalid_response",
            `the API answered ${res.status} but the response is larger than ${MAX_RESPONSE_BYTES / (1 << 20)} MB`,
            res.status,
          );
        text = "";
      } else text = read;
    } catch (e) {
      if (e instanceof Unavailable) throw e;
      // The answer began and then broke: the per-attempt timeout fired while the body was still arriving, or the connection
      // dropped. A success whose content we never saw is a lost answer (Unavailable, retried with the same id as the class
      // allows); an error status is still classified by its status, as an error with no body is.
      if (res.ok) throw new Unavailable("unavailable", "the Vunly API answer was cut off", 0);
      text = "";
    }
    let parsed: unknown;
    try {
      parsed = text ? JSON.parse(text) : undefined;
    } catch {
      parsed = undefined;
    }
    if (!res.ok) {
      const e = (
        parsed as
          | {
              error?: {
                code?: ErrorCode;
                message?: string;
                requestId?: string;
                details?: Record<string, unknown>;
              };
            }
          | undefined
      )?.error;
      // an error with no body (a proxy, an empty 5xx): the code follows the status, not "internal_error" for everything.
      // Everything the server wrote is cleaned, bounded and stripped of the key before it reaches the caller.
      const key = options.apiKey;
      const rawCode = typeof e?.code === "string" ? scrub(e.code, MAX_CODE_CHARS, key) : "";
      const code = (rawCode || fallbackCode(res.status)) as ErrorCode;
      const Err = classify(res.status, code);
      const message =
        typeof e?.message === "string" && scrub(e.message, MAX_MESSAGE_CHARS, key)
          ? scrub(e.message, MAX_MESSAGE_CHARS, key)
          : `request failed with status ${res.status}`;
      const details =
        e?.details !== null && typeof e?.details === "object" && !Array.isArray(e.details)
          ? (scrubValue(e.details, key) as Record<string, unknown>)
          : undefined;
      const error = new Err(
        code,
        res.status === 404 && /no such route/i.test(message) ? message + WRONG_ROUTE_HINT : message,
        res.status,
        scrubId(e?.requestId, key),
        details,
      );
      const ra = parseRetryAfter(res.headers.get("Retry-After"));
      if (error instanceof Unavailable && ra !== undefined) error.retryAfterSeconds = ra;
      throw error;
    }
    if (shape) {
      const problem = shapeProblem(parsed, shape);
      if (problem)
        // A 2xx whose body lacks a promised field: fail closed with the SDK's own error, never a silent `undefined`.
        // Not retried (the status is not retryable); the call may have been applied, so a later retry with the SAME id is safe.
        throw new Unavailable(
          "invalid_response",
          `the API answered ${res.status} but the response is malformed (${problem})`,
          res.status,
          scrubId(res.headers.get("request-id") ?? undefined, options.apiKey),
        );
    }
    return {
      status: res.status,
      replayed: res.headers.get("Idempotent-Replayed") === "true",
      data: parsed as T,
      text,
      headers: res.headers,
    };
  }

  const enc = encodeURIComponent;

  async function exportCsv(
    which: "ledger" | "usage" | "customers",
    input: ExportOptions = {},
  ): Promise<CsvExport> {
    const { customer, ...rest } = input;
    const q = new URLSearchParams();
    const all: Record<string, unknown> = { ...rest, ...(customer ? { customerId: customer } : {}) };
    for (const [k, v] of Object.entries(all)) if (v !== undefined) q.set(k, String(v));
    const r = await call<unknown>("GET", `/v1/exports/${which}.csv${q.size ? `?${q}` : ""}`);
    return { csv: r.text, nextCursor: r.headers.get("x-next-cursor") };
  }
  return {
    customers: {
      upsert: async ({ id, name }) =>
        (
          await call<Customer>(
            "POST",
            "/v1/customers",
            { externalId: id, ...(name === undefined ? {} : { name }) },
            undefined,
            { id: "string", externalId: "string", createdAt: "string" },
            "N", // an upsert: asking twice leaves the same customer
          )
        ).data,
      get: async ({ customer }) =>
        (
          await call<Customer>(
            "GET",
            `/v1/customers/${encodeURIComponent(customer)}`,
            undefined,
            undefined,
            { id: "string", externalId: "string", createdAt: "string" },
          )
        ).data,
      list: async ({ limit, cursor } = {}) => {
        const q = new URLSearchParams();
        if (limit !== undefined) q.set("limit", String(limit));
        if (cursor !== undefined) q.set("cursor", cursor);
        return (
          await call<{ data: Customer[]; nextCursor: string | null }>(
            "GET",
            `/v1/customers${q.size ? `?${q}` : ""}`,
            undefined,
            undefined,
            LIST_SHAPE,
          )
        ).data;
      },
    },
    entitlements: {
      check: async ({ customer, feature, estimate }) =>
        (
          await call<Entitlement>(
            "POST",
            "/v1/entitlements/check",
            { customerId: customer, feature, ...(estimate === undefined ? {} : { estimate }) },
            undefined,
            { allowed: "boolean", reason: "string", remaining: "number", required: "number" },
            "R", // read-only and advisory
          )
        ).data,
    },
    plans: {
      upsertMeter: async ({ key, unitsPerCredit }) => {
        await call("POST", "/v1/meters", { key, unitsPerCredit }, undefined, undefined, "N");
      },
      upsert: async (input) =>
        (await call<Plan>("POST", "/v1/plans", input, undefined, PLAN_SHAPE, "N")).data,
      subscribe: async ({ customer, plan }) =>
        (
          await call<Subscription>(
            "POST",
            `/v1/customers/${enc(customer)}/subscription`,
            { plan },
            undefined,
            SUBSCRIPTION_SHAPE,
            "N", // the same plan again is a 200 with the existing subscription: asking twice leaves one
          )
        ).data,
      cancel: async ({ customer }) => {
        await call("DELETE", `/v1/customers/${enc(customer)}/subscription`);
      },
      change: async ({ id, customer, plan, timing }) => {
        const r = await call<{ change: SubscriptionChange | null; subscription: Subscription }>(
          "POST",
          `/v1/customers/${enc(customer)}/subscription/change`,
          { plan, ...(timing === undefined ? {} : { timing }) },
          id,
          prefixed("subscription.", SUBSCRIPTION_SHAPE),
        );
        return { ...r.data, replayed: r.replayed };
      },
      cancelPendingChange: async ({ customer }) => {
        await call("DELETE", `/v1/customers/${enc(customer)}/subscription/pending-change`);
      },
    },
    exports: {
      ledger: (input) => exportCsv("ledger", input),
      usage: (input) => exportCsv("usage", input),
      customers: (input) => exportCsv("customers", input),
    },
    customerTokens: {
      create: async ({ customer, ttlSeconds }) =>
        (
          await call<{ id: string; token: string; customerId: string; expiresAt: string }>(
            "POST",
            "/v1/customer-tokens",
            { customerId: customer, ...(ttlSeconds === undefined ? {} : { ttlSeconds }) },
            undefined,
            { id: "string", token: "string", customerId: "string", expiresAt: "string" },
          )
        ).data,
      revoke: async ({ id }) => {
        await call("DELETE", `/v1/customer-tokens/${enc(id)}`);
      },
    },
    billing: {
      diagnostics: async () =>
        (
          await call<BillingDiagnostics>("GET", "/v1/billing/diagnostics", undefined, undefined, {
            outbox: "object",
            webhooks: "object",
            reconciliation: "object",
          })
        ).data,
      reconciliation: {
        list: async (input = {}) => {
          const { customer, ...rest } = input;
          const q = new URLSearchParams();
          const all: Record<string, unknown> = {
            ...rest,
            ...(customer ? { customerId: customer } : {}),
          };
          for (const [k, v] of Object.entries(all)) if (v !== undefined) q.set(k, String(v));
          return (
            await call<{ data: ReconciliationItem[]; nextCursor: string | null }>(
              "GET",
              `/v1/reconciliation/items${q.size ? `?${q}` : ""}`,
              undefined,
              undefined,
              LIST_SHAPE,
            )
          ).data;
        },
        run: async () =>
          (await call<Record<string, unknown>>("POST", "/v1/reconciliation/run", {}, undefined, {}))
            .data,
        resend: async ({ id }) =>
          (
            await call<{ item: ReconciliationItem; requeued: number }>(
              "POST",
              `/v1/reconciliation/items/${enc(id)}/resend`,
              {},
              undefined,
              { requeued: "number", "item.id": "string" },
            )
          ).data,
      },
    },
    creditPacks: {
      upsert: async (input) =>
        (await call<CreditPack>("POST", "/v1/credit-packs", input, undefined, PACK_SHAPE, "N"))
          .data,
      list: async () =>
        (
          await call<{ data: CreditPack[] }>(
            "GET",
            "/v1/credit-packs",
            undefined,
            undefined,
            LIST_SHAPE,
          )
        ).data.data,
      purchase: async ({ id, customer, pack }) => {
        const r = await call<{ entry: LedgerEntry; balance: number }>(
          "POST",
          `/v1/customers/${enc(customer)}/credit-packs/${enc(pack)}/purchase`,
          undefined,
          id,
          { balance: "number", ...prefixed("entry.", ENTRY_SHAPE) },
        );
        return { ...r.data, replayed: r.replayed };
      },
    },
    recipes: {
      list: async () =>
        (await call<{ data: Recipe[] }>("GET", "/v1/recipes", undefined, undefined, LIST_SHAPE))
          .data.data,
      apply: async ({ recipe, params }) =>
        (
          await call<AppliedRecipe>(
            "POST",
            `/v1/recipes/${enc(recipe)}/apply`,
            params === undefined ? {} : { params },
            undefined,
            { "recipe.id": "string", plan: "object", created: "object" },
          )
        ).data,
    },
    webhooks: {
      endpoints: {
        create: async (input) =>
          (
            await call<WebhookEndpoint & { secret: string }>(
              "POST",
              "/v1/webhook-endpoints",
              input,
              undefined,
              { ...ENDPOINT_SHAPE, secret: "string" },
            )
          ).data,
        list: async () =>
          (
            await call<{ data: WebhookEndpoint[] }>(
              "GET",
              "/v1/webhook-endpoints",
              undefined,
              undefined,
              LIST_SHAPE,
            )
          ).data.data,
        update: async ({ id, ...patch }) =>
          (
            await call<WebhookEndpoint>(
              "PATCH",
              `/v1/webhook-endpoints/${enc(id)}`,
              patch,
              undefined,
              ENDPOINT_SHAPE,
            )
          ).data,
        delete: async ({ id }) => {
          await call("DELETE", `/v1/webhook-endpoints/${enc(id)}`);
        },
        rotateSecret: async ({ id }) =>
          (
            await call<WebhookEndpoint & { secret: string }>(
              "POST",
              `/v1/webhook-endpoints/${enc(id)}/rotate-secret`,
              {},
              undefined,
              { ...ENDPOINT_SHAPE, secret: "string" },
            )
          ).data,
        test: async ({ id }) =>
          (
            await call<WebhookDelivery>(
              "POST",
              `/v1/webhook-endpoints/${enc(id)}/test`,
              {},
              undefined,
              DELIVERY_SHAPE,
            )
          ).data,
      },
      deliveries: {
        list: async (input = {}) => {
          const q = new URLSearchParams();
          for (const [k, v] of Object.entries(input)) if (v !== undefined) q.set(k, String(v));
          const qs = q.size ? `?${q}` : "";
          return (
            await call<{ data: WebhookDelivery[]; nextCursor: string | null }>(
              "GET",
              `/v1/webhook-deliveries${qs}`,
              undefined,
              undefined,
              LIST_SHAPE,
            )
          ).data;
        },
        get: async ({ id }) =>
          (
            await call<WebhookDeliveryDetail>(
              "GET",
              `/v1/webhook-deliveries/${enc(id)}`,
              undefined,
              undefined,
              { ...DELIVERY_SHAPE, event: "object", attemptLog: "array" },
            )
          ).data,
        redeliver: async ({ id }) =>
          (
            await call<WebhookDeliveryDetail>(
              "POST",
              `/v1/webhook-deliveries/${enc(id)}/redeliver`,
              {},
              undefined,
              { ...DELIVERY_SHAPE, event: "object", attemptLog: "array" },
            )
          ).data,
      },
    },
    activity: {
      list: async (input = {}) => {
        const { customer, ...rest } = input;
        const q = new URLSearchParams();
        const all: Record<string, unknown> = {
          ...rest,
          ...(customer ? { customerId: customer } : {}),
        };
        for (const [k, v] of Object.entries(all)) if (v !== undefined) q.set(k, String(v));
        return (
          await call<{ data: ActivityItem[]; nextCursor: string | null; retentionDays: number }>(
            "GET",
            `/v1/activity${q.size ? `?${q}` : ""}`,
            undefined,
            undefined,
            { data: "array", retentionDays: "number" },
          )
        ).data;
      },
    },
    usage: {
      record: async ({ id, customer, meter, quantity }) => {
        const r = await call<{ event: LedgerEntry; balance: number }>(
          "POST",
          "/v1/usage-events",
          { customerId: customer, meter, quantity },
          id,
          { balance: "number", ...prefixed("event.", ENTRY_SHAPE) },
        );
        return {
          event: r.data.event,
          remaining: r.data.balance,
          replayed: r.replayed,
          planOverage: !r.replayed && r.headers.get("Vunly-Plan-Overage") === "true",
        };
      },
    },
    planUsage: async () =>
      (
        await call<PlanUsage>("GET", "/v1/plan-usage", undefined, undefined, {
          "plan.key": "string",
          "period.start": "string",
          "usage.events": "number",
          "unconfirmedLimits.confirmed": "boolean",
        })
      ).data,
    credits: {
      balance: async ({ customer }) => {
        const r = await call<{ credits: number; consumed: number; granted: number }>(
          "GET",
          `/v1/customers/${enc(customer)}/balance`,
          undefined,
          undefined,
          { credits: "number", consumed: "number", granted: "number" },
        );
        return { remaining: r.data.credits, consumed: r.data.consumed, granted: r.data.granted };
      },
      ledger: async ({ customer, limit, cursor }) => {
        const q = new URLSearchParams();
        if (limit !== undefined) q.set("limit", String(limit));
        if (cursor !== undefined) q.set("cursor", cursor);
        return (
          await call<Page<LedgerEntry>>(
            "GET",
            `/v1/customers/${enc(customer)}/ledger${q.size ? `?${q}` : ""}`,
            undefined,
            undefined,
            LIST_SHAPE,
          )
        ).data;
      },
      usage: async ({ customer }) =>
        (
          await call<UsageSummary>(
            "GET",
            `/v1/customers/${enc(customer)}/usage`,
            undefined,
            undefined,
            { customerId: "string", ...USAGE_SHAPE },
          )
        ).data,
      grant: async ({ id, customer, credits, reason }) =>
        (
          await call<LedgerEntry>(
            "POST",
            `/v1/customers/${enc(customer)}/credit-grants`,
            { credits, ...(reason === undefined ? {} : { reason }) },
            id,
            ENTRY_SHAPE,
          )
        ).data,
    },
  };
}
