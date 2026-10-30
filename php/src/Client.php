<?php

declare(strict_types=1);

namespace Vunly;

use Vunly\Http\CurlTransport;
use Vunly\Http\Transport;
use Vunly\Models\PlanUsage;
use Vunly\Services\CreditsService;
use Vunly\Services\CustomersService;
use Vunly\Services\CustomerTokensService;
use Vunly\Services\EntitlementsService;
use Vunly\Services\PlansService;
use Vunly\Services\UsageService;

/**
 * The client for the Vunly API: customers, credits, usage, entitlement checks, plans and customer tokens. Sandbox only
 * for now. The only requirements are PHP 8.2+ with ext-curl and ext-json.
 *
 * The loop every integration is built from:
 *
 *     $vunly = new Client(apiKey: getenv('VUNLY_API_KEY'), baseUrl: getenv('VUNLY_API_URL'));
 *     $ent = $vunly->entitlements->check('acme', 'generate');   // throws Unavailable: treat it as "no"
 *     if (!$ent->allowed) { // do not run the work }
 *     // ... run the work ...
 *     $res = $vunly->usage->record(id: $requestId, customer: 'acme', meter: 'tokens', quantity: 1050);
 *
 * The check is advisory and fails closed; the record is final. The id names the unit of work (your request or job id),
 * is sent as the Idempotency-Key, and is never generated for you: retry with the SAME one.
 *
 * A client is stateless between calls and safe to share. PHP is synchronous: a retry wait blocks the process for that
 * time (at most 30 seconds), so set `maxRetries: 0` where a request must not wait.
 */
final class Client implements \Stringable
{
    public const VERSION = '0.1.0';

    /** The largest response body accepted, in bytes. */
    public const MAX_BODY_BYTES = Json::MAX_BYTES;

    /** The largest body read from a non-2xx answer, in bytes: only its status and a short error matter. */
    public const MAX_ERROR_BODY_BYTES = 65_536;

    /** The longest Retry-After the client waits for, in seconds; a longer one is raised, not waited on. */
    public const MAX_RETRY_AFTER_SECONDS = 30.0;

    /** What an error reports for a Retry-After beyond the cap (or too large to represent): the value capped here. */
    public const MAX_REPORTED_RETRY_AFTER_SECONDS = 86400.0;

    /** Paging fails with `invalid_response` on the THIRD consecutive empty page that still promises another (a next cursor). */
    public const MAX_EMPTY_PAGES = 3;

    /** The accepted range of the per-attempt timeout, in seconds. */
    public const MIN_TIMEOUT_SECONDS = 0.05;
    public const MAX_TIMEOUT_SECONDS = 600.0;

    public readonly CustomersService $customers;
    public readonly CreditsService $credits;
    public readonly UsageService $usage;
    public readonly EntitlementsService $entitlements;
    public readonly PlansService $plans;
    public readonly CustomerTokensService $customerTokens;

    private readonly Executor $executor;
    private readonly string $baseUrl;

    /**
     * @param string         $apiKey     required; read it from your environment (VUNLY_API_KEY), never from source
     * @param string         $baseUrl    required; the API address, e.g. "https://host" or "http://127.0.0.1:8787". A trailing
     *                                   slash or "/v1" is accepted and ignored. There is no default address.
     * @param float          $timeout    seconds for ONE attempt (connect, headers and body together); default 10
     * @param int            $maxRetries how many times a retryable failure is repeated; default 2, 0 turns retries off
     * @param Transport|null $transport  replaces the HTTP layer (tests, special proxies); it must follow the Transport rules
     * @param \Closure|null  $sleeper    replaces the wait between retries; receives seconds as a float (tests)
     */
    public function __construct(
        #[\SensitiveParameter]
        string $apiKey,
        #[\SensitiveParameter]
        string $baseUrl,
        float $timeout = 10.0,
        int $maxRetries = 2,
        ?Transport $transport = null,
        ?\Closure $sleeper = null,
    ) {
        // A trailing line break from an env file or a shell is removed; anything else that is not a printable, space-free ASCII
        // token (a CR LF inside would inject a header) is refused. The message never contains the value.
        // Only ONE trailing line break (LF or CRLF) is removed. Spaces, tabs, NUL and a lone CR, at either end, are not.
        $apiKey = (string) preg_replace('/\r?\n\z/', '', $apiKey, 1);
        if ($apiKey === '') {
            throw new \InvalidArgumentException('Vunly: the API key is empty (read it from your environment).');
        }
        if (preg_match('/^[\x21-\x7E]+$/D', $apiKey) !== 1) {
            throw new \InvalidArgumentException('Vunly: the API key must be printable ASCII without spaces or control characters (check how VUNLY_API_KEY is set).');
        }
        if (!is_finite($timeout) || $timeout < self::MIN_TIMEOUT_SECONDS || $timeout > self::MAX_TIMEOUT_SECONDS) {
            throw new \InvalidArgumentException(sprintf('Vunly: the timeout must be between %s and %s seconds.', self::MIN_TIMEOUT_SECONDS, self::MAX_TIMEOUT_SECONDS));
        }
        if ($maxRetries < 0) {
            throw new \InvalidArgumentException('Vunly: maxRetries must not be negative.');
        }
        $this->baseUrl = self::normalizeBaseUrl($baseUrl);
        $this->executor = new Executor(
            new Secret($apiKey),
            $this->baseUrl,
            $timeout,
            $maxRetries,
            $transport ?? new CurlTransport(),
            $sleeper ?? static function (float $seconds): void {
                if ($seconds > 0) {
                    usleep((int) round($seconds * 1_000_000));
                }
            },
            sprintf('vunly-php/%s (PHP %s; %s)', self::VERSION, PHP_VERSION, PHP_OS_FAMILY),
        );

        $this->customers = new CustomersService($this->executor);
        $this->credits = new CreditsService($this->executor);
        $this->usage = new UsageService($this->executor);
        $this->entitlements = new EntitlementsService($this->executor);
        $this->plans = new PlansService($this->executor);
        $this->customerTokens = new CustomerTokensService($this->executor);
    }

    /**
     * The project's plan and this month's recorded usage. Its limits are inside `unconfirmedLimits` and are not a
     * guarantee (`confirmed` is false).
     */
    public function planUsage(): PlanUsage
    {
        return $this->executor->fetch(
            'GET',
            '/v1/plan-usage',
            PlanUsage::fromArray(...),
            ['plan.key' => 'string', 'period.start' => 'string', 'usage.events' => 'int', 'unconfirmedLimits.confirmed' => 'bool'],
        )->value;
    }

    // The same services as the public properties, as methods: a Laravel facade forwards method calls only
    // (`Vunly::customers()->upsert(...)`).

    public function customers(): CustomersService
    {
        return $this->customers;
    }

    public function credits(): CreditsService
    {
        return $this->credits;
    }

    public function usage(): UsageService
    {
        return $this->usage;
    }

    public function entitlements(): EntitlementsService
    {
        return $this->entitlements;
    }

    public function plans(): PlansService
    {
        return $this->plans;
    }

    public function customerTokens(): CustomerTokensService
    {
        return $this->customerTokens;
    }

    /** Never contains the key. */
    public function __toString(): string
    {
        return sprintf('Vunly\\Client{baseUrl: %s, apiKey: %s}', $this->baseUrl, Secret::PLACEHOLDER);
    }

    /** @return array<string, mixed> */
    public function __debugInfo(): array
    {
        return ['baseUrl' => $this->baseUrl, 'apiKey' => Secret::PLACEHOLDER];
    }

    public function __serialize(): array
    {
        throw new \LogicException('A Vunly client cannot be serialized (it holds the API key).');
    }

    /** @param array<mixed> $data */
    public function __unserialize(array $data): void
    {
        throw new \LogicException('A Vunly client cannot be unserialized.');
    }

    /** The address without a trailing slash and without a trailing "/v1": the SDK adds "/v1/..." itself. */
    public static function normalizeBaseUrl(#[\SensitiveParameter] string $url): string
    {
        $out = rtrim(trim($url), '/');
        $out = rtrim((string) preg_replace('~/v1$~i', '', $out), '/');
        if ($out === '') {
            throw new \InvalidArgumentException('Vunly: the base URL is empty (use the API address from the setup step).');
        }
        $parts = parse_url($out);
        $host = is_array($parts) ? ($parts['host'] ?? '') : '';
        $port = is_array($parts) ? ($parts['port'] ?? null) : null;
        $wellFormed = is_array($parts)
            && preg_match('~^https?://[\x21-\x7E]+$~Di', $out) === 1 // printable ASCII, no spaces or control characters
            && in_array($parts['scheme'] ?? '', ['http', 'https'], true)
            && $host !== ''
            && (preg_match('/^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$/D', $host) === 1 || preg_match('/^\[[0-9A-Fa-f:.]+\]$/D', $host) === 1)
            && ($port === null || ($port >= 1 && $port <= 65535));
        if (!$wellFormed) {
            throw new \InvalidArgumentException('Vunly: the base URL must be an absolute http(s) address such as https://host.');
        }
        if (isset($parts['user']) || isset($parts['pass']) || isset($parts['query']) || isset($parts['fragment'])) {
            // Never echo the value: it could be the credentials this check exists to keep out of messages.
            throw new \InvalidArgumentException('Vunly: the base URL must not contain credentials, a query or a fragment.');
        }

        return $out;
    }

    /**
     * Refuses, before any request, an id that could not be an Idempotency-Key: the same error the API would give.
     *
     * @internal
     */
    public static function checkId(mixed $id): void
    {
        if (!is_string($id)) {
            // the type, never the value
            throw new InvalidRequest(
                'id must be a string of 8 to 128 characters (it is sent as the Idempotency-Key), got ' . get_debug_type($id),
                'validation_error',
                400,
                null,
                ['fields' => ['id' => 'must be a string, got ' . get_debug_type($id)]],
            );
        }
        if (preg_match('/^[A-Za-z0-9_.:-]{8,128}$/D', $id) !== 1) {
            throw new InvalidRequest(
                'id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)',
                'validation_error',
                400,
                null,
                ['fields' => ['id' => '8-128 chars of A-Za-z0-9_.:-']],
            );
        }
    }

    /**
     * Escapes one path segment, refusing an empty one (it would address another route) and "." / "..".
     *
     * @internal
     */
    public static function segment(mixed $value, string $what): string
    {
        if (!is_string($value)) {
            // `mixed` on purpose: a null or an int is an InvalidRequest naming the problem (the type, never the value), not a TypeError
            throw new InvalidRequest("$what must be a string, got " . get_debug_type($value), 'validation_error', 400, null, ['fields' => [$what => 'must be a string, got ' . get_debug_type($value)]]);
        }
        if ($value === '') {
            throw new InvalidRequest("$what must not be empty", 'validation_error', 400, null, ['fields' => [$what => 'required']]);
        }
        if ($value === '.' || $value === '..') {
            throw new InvalidRequest("$what must not be \".\" or \"..\" (they would change the path that is called)", 'validation_error', 400, null, ['fields' => [$what => 'must not be "." or ".."']]);
        }

        return rawurlencode($value);
    }
}
