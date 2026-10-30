// Receiving Vunly webhooks: event types and signature verification. Works in Node 22+, edge runtimes and browsers'
// service workers (Web Crypto only). Verify the RAW request body, before parsing it.

export const WEBHOOK_EVENT_TYPES = [
  "credits.low",
  "credits.exhausted",
  "usage.limit_reached",
  "subscription.activated",
] as const;
export type WebhookEventType = (typeof WEBHOOK_EVENT_TYPES)[number];

interface EventBase<T extends string, D> {
  /** Stable for the life of the event: identical on every retry and every redelivery. Dedupe on this. */
  id: string;
  type: T;
  createdAt: string;
  environment: "sandbox" | "production";
  data: D;
}
interface CustomerRef {
  /** Vunly's customer id (`cus_...`). */
  customerId: string;
  /** The id you gave `customers.upsert`. */
  externalId: string;
}

/** The balance fell to 10% or less of everything granted (and is above zero). Fires once per downward crossing. */
export type CreditsLowEvent = EventBase<
  "credits.low",
  CustomerRef & { balance: number; granted: number; threshold: number }
>;
/** The balance reached zero. */
export type CreditsExhaustedEvent = EventBase<
  "credits.exhausted",
  CustomerRef & { balance: 0; granted: number }
>;
/** A usage record was refused for lack of credits. At most one per customer per 10 minutes. */
export type UsageLimitReachedEvent = EventBase<
  "usage.limit_reached",
  CustomerRef & { meter: string; balance: number; required: number }
>;
export type SubscriptionActivatedEvent = EventBase<
  "subscription.activated",
  CustomerRef & { subscriptionId: string; plan: string; periodStart: string; periodEnd: string }
>;
/** Sent by `webhooks.endpoints.test` only. */
export type WebhookTestEvent = EventBase<"webhook.test", { endpointId: string }>;

export type VunlyEvent =
  | CreditsLowEvent
  | CreditsExhaustedEvent
  | UsageLimitReachedEvent
  | SubscriptionActivatedEvent
  | WebhookTestEvent;

export type SignatureFailure =
  | "missing_header"
  | "malformed_header"
  | "stale_timestamp"
  | "no_matching_signature"
  | "invalid_payload";

export class WebhookSignatureError extends Error {
  readonly reason: SignatureFailure;
  constructor(reason: SignatureFailure) {
    super(`webhook rejected: ${reason}`);
    this.name = "WebhookSignatureError";
    this.reason = reason;
  }
}

const enc = new TextEncoder();
const DEFAULT_TOLERANCE_SECONDS = 300;

async function hmacHex(secret: string, message: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(message)));
  return [...sig].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function safeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

/**
 * Checks the `Vunly-Signature` header against the raw body and returns the parsed event, or throws
 * `WebhookSignatureError`. Pass every secret that may be live (two during a rotation). The timestamp must be within
 * `toleranceSeconds` (default 300) of now, which is what stops a captured request being replayed later.
 *
 * Respond 2xx quickly (do the work after answering). Anything else is retried with backoff for about two hours, so
 * the same event WILL arrive more than once: keep the ids you have processed and skip repeats.
 *
 * ```ts
 * const event = await verifyWebhook({
 *   payload: rawBody, // the exact bytes received, as a string
 *   signature: request.headers.get("vunly-signature"),
 *   secret: process.env.VUNLY_WEBHOOK_SECRET!,
 * });
 * if (await alreadyProcessed(event.id)) return new Response(null, { status: 200 });
 * ```
 */
export async function verifyWebhook(input: {
  payload: string;
  signature: string | null | undefined;
  secret: string | string[];
  toleranceSeconds?: number;
  /** Test hook: the current time (a Date, or milliseconds since the epoch). */
  now?: Date | number;
}): Promise<VunlyEvent> {
  const tolerance = input.toleranceSeconds ?? DEFAULT_TOLERANCE_SECONDS;
  if (!(tolerance > 0)) throw new Error("toleranceSeconds must be positive");
  // An empty secret signs anything anyone wants: a missing environment variable would make webhooks forgeable. A configuration
  // error, raised before any signature is looked at (a WebhookSignatureError would be answered 400 and hide it).
  const configured = Array.isArray(input.secret) ? input.secret : [input.secret];
  // A usable secret has at least one printable ASCII character that is not a space (HMAC-SHA256, 300 s tolerance).
  if (
    configured.length === 0 ||
    configured.some((s) => typeof s !== "string" || !/[\x21-\x7e]/.test(s))
  )
    throw new Error(
      "verifyWebhook: the webhook secret is empty or missing (is its environment variable set?)",
    );
  if (!input.signature || input.signature.trim() === "")
    throw new WebhookSignatureError("missing_header");
  let t: number | null = null;
  const sigs: string[] = [];
  for (const part of input.signature.split(",")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    const k = part.slice(0, eq).trim();
    const v = part.slice(eq + 1).trim();
    if (k === "t" && /^\d{1,12}$/.test(v)) t = Number(v);
    else if (k === "v1" && /^[0-9a-f]{64}$/.test(v)) sigs.push(v);
  }
  if (t === null || sigs.length === 0) throw new WebhookSignatureError("malformed_header");
  const nowMs = input.now === undefined ? Date.now() : Number(input.now);
  const nowSeconds = Math.floor(nowMs / 1000);
  if (Math.abs(nowSeconds - t) > tolerance) throw new WebhookSignatureError("stale_timestamp");
  const secrets = Array.isArray(input.secret) ? input.secret : [input.secret];
  let ok = false;
  for (const secret of secrets) {
    const expected = await hmacHex(secret, `${t}.${input.payload}`);
    if (sigs.some((s) => safeEqual(s, expected))) ok = true;
  }
  if (!ok) throw new WebhookSignatureError("no_matching_signature");
  try {
    const event = JSON.parse(input.payload) as VunlyEvent;
    if (typeof event?.id !== "string" || typeof event.type !== "string") throw new Error("shape");
    return event;
  } catch {
    throw new WebhookSignatureError("invalid_payload");
  }
}
