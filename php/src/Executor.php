<?php

declare(strict_types=1);

namespace Vunly;

use Vunly\Http\Failure;
use Vunly\Http\Request;
use Vunly\Http\Response;
use Vunly\Http\Transport;

/**
 * Runs one API call with the retry rules of its operation class. It is what the services talk to; it holds the key and the
 * transport, and it does not know the services, so dumping a {@see Client} has no circular reference.
 *
 * @internal
 */
final class Executor
{
    public function __construct(
        private readonly Secret $apiKey,
        private readonly string $baseUrl,
        private readonly float $timeout,
        private readonly int $maxRetries,
        private readonly Transport $transport,
        private readonly \Closure $sleeper,
        private readonly string $userAgent,
    ) {
    }

    /** @return array<string, mixed> */
    public function __debugInfo(): array
    {
        return ['baseUrl' => $this->baseUrl, 'apiKey' => Secret::PLACEHOLDER];
    }

    /**
     * A call whose answer is not read (an upsert of a meter, a delete): only the status matters.
     *
     * @param array<string, mixed>|null $body
     *
     * @return Reply<null>
     */
    public function call(string $method, string $path, ?array $body = null, OpClass $class = OpClass::Natural): Reply
    {
        /** @var Reply<null> $reply */
        $reply = $this->run($method, $path, [], $body, null, $class, [], null);

        return $reply;
    }

    /**
     * `shape` lists the fields a successful body must carry ("a.b" => "string|int|bool|array|object", suffix "?" = may be
     * null or absent). `map` builds the result from the decoded body; anything it throws is an `invalid_response`.
     *
     * @template T
     *
     * @param array<string, scalar|null>      $query
     * @param array<string, mixed>|null       $body
     * @param array<string, string>           $shape
     * @param \Closure(array<string, mixed>): T $map
     *
     * @return Reply<T>
     */
    public function fetch(
        string $method,
        string $path,
        \Closure $map,
        array $shape,
        array $query = [],
        ?array $body = null,
        ?string $idem = null,
        OpClass $class = OpClass::Read,
    ): Reply {
        /** @var Reply<T> $reply */
        $reply = $this->run($method, $path, $query, $body, $idem, $class, $shape, $map);

        return $reply;
    }

    /**
     * @param array<string, scalar|null>                   $query
     * @param array<string, mixed>|null                    $body
     * @param array<string, string>                        $shape
     * @param (\Closure(array<string, mixed>): mixed)|null $map
     *
     * @return Reply<mixed>
     */
    private function run(string $method, string $path, array $query, ?array $body, ?string $idem, OpClass $class, array $shape, ?\Closure $map): Reply
    {
        if ($idem !== null) {
            Client::checkId($idem);
        }
        $url = $this->baseUrl . $path;
        $query = array_filter($query, static fn ($v) => $v !== null);
        if ($query !== []) {
            $url .= '?' . http_build_query($query, '', '&', PHP_QUERY_RFC3986);
        }
        $payload = $body === null ? null : json_encode($body, JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);

        for ($attempt = 1;; $attempt++) {
            try {
                return $this->once($method, $url, $payload, $idem, $shape, $map);
            } catch (Unavailable $e) {
                if ($attempt > $this->maxRetries || !self::mayRetry($class, $e)) {
                    throw $e;
                }
                $wait = self::retryDelay($attempt, $e->retryAfterSeconds);
                if ($wait === null) {
                    throw $e; // a Retry-After beyond the cap: surface it instead of hanging the request
                }
                ($this->sleeper)($wait);
            }
        }
    }

    /**
     * The retry classes: R, K and N are repeated after a lost answer, a timeout or a 5xx; X only after a 429, which was
     * refused before it ran. The plan-volume stop and `invalid_response` are never repeated.
     */
    private static function mayRetry(OpClass $class, Unavailable $e): bool
    {
        if (!in_array($e->status, [0, 429, 500, 502, 503, 504], true) || $e->isPlanStop()) {
            return false;
        }

        return !($class === OpClass::Other && $e->status !== 429);
    }

    /** min(8 s, 0.25 s x 2^(n-1)) times a random factor between 0.5 and 1; Retry-After instead, up to the cap. */
    private static function retryDelay(int $attempt, ?float $retryAfter): ?float
    {
        if ($retryAfter !== null && $retryAfter > 0) {
            return $retryAfter <= Client::MAX_RETRY_AFTER_SECONDS ? $retryAfter : null;
        }
        $base = min(8.0, 0.25 * (2 ** ($attempt - 1)));

        return $base * (0.5 + (mt_rand() / mt_getrandmax()) / 2);
    }

    /**
     * @param array<string, string>                        $shape
     * @param (\Closure(array<string, mixed>): mixed)|null $map
     *
     * @return Reply<mixed>
     */
    private function once(string $method, string $url, ?string $payload, ?string $idem, array $shape, ?\Closure $map): Reply
    {
        $headers = [
            'Authorization' => 'Bearer ' . $this->apiKey->reveal(),
            'User-Agent' => $this->userAgent,
            'Accept' => 'application/json',
        ];
        if ($payload !== null) {
            $headers['Content-Type'] = 'application/json';
        }
        if ($idem !== null) {
            $headers['Idempotency-Key'] = $idem;
        }

        $res = $this->transport->send(new Request($method, $url, $headers, $payload, $this->timeout, Client::MAX_BODY_BYTES, Client::MAX_ERROR_BODY_BYTES));
        $requestId = $res->header('request-id');
        $requestId = $requestId === null ? null : self::clean($requestId, 128);
        $requestId = $requestId === '' ? null : $requestId;

        if ($res->failure === Failure::Timeout) {
            throw new Unavailable(sprintf('the Vunly API did not answer within %s seconds', rtrim(rtrim(sprintf('%.3f', $this->timeout), '0'), '.')), 'unavailable', 0);
        }
        if ($res->failure === Failure::Connection) {
            // Deliberately without the library's own text: it can contain the address or headers.
            throw new Unavailable('could not reach the Vunly API, or the answer was cut off', 'unavailable', 0);
        }
        $success = $res->status >= 200 && $res->status < 300;
        if ($res->failure === Failure::TooLarge) {
            if ($success) {
                throw self::invalidResponse($res->status, $requestId, 'the body is larger than ' . (Client::MAX_BODY_BYTES / 1048576) . ' MiB, the most that is read');
            }
            $this->fail($res, $res->body, $requestId); // the first 64 KB of the error, parsed as far as they go
        }
        if (!$success) {
            $this->fail($res, $res->body, $requestId);
        }

        $replayed = $res->header('idempotent-replayed') === 'true';
        $overage = $res->header('vunly-plan-overage') === 'true' && !$replayed;
        if ($shape === [] && $map === null) {
            return new Reply(null, $res->status, $requestId, $replayed, $overage);
        }

        try {
            $data = Json::decode($res->body);
        } catch (\JsonException) {
            throw self::invalidResponse($res->status, $requestId, 'the body is not JSON');
        } catch (\UnexpectedValueException $e) {
            // Json's own message names a size and a count, never any content
            throw self::invalidResponse($res->status, $requestId, $e->getMessage());
        }
        if (!is_array($data) || (array_is_list($data) && $data !== [])) {
            throw self::invalidResponse($res->status, $requestId, 'the body is not a JSON object');
        }
        $problem = self::shapeProblem($data, $shape);
        if ($problem !== null) {
            throw self::invalidResponse($res->status, $requestId, $problem);
        }
        try {
            /** @var array<string, mixed> $data */
            $value = $map === null ? $data : $map($data);
        } catch (\Throwable) {
            throw self::invalidResponse($res->status, $requestId, 'unexpected shape');
        }

        return new Reply($value, $res->status, $requestId, $replayed, $overage);
    }

    /** Throws the typed error for a non-2xx answer. An error with no JSON body (a proxy, an empty 5xx) is classified by its status. */
    private function fail(Response $res, string $raw, ?string $requestId): never
    {
        if ($res->status >= 300 && $res->status < 400) {
            // Redirects are never followed (the key would go to another host), and a 3xx is not a client mistake: the
            // address is wrong or something in front of the API answered. Unavailable, and not retried (status not retryable).
            throw new Unavailable(
                sprintf('the API answered %d, a redirect, which is not followed (check the API address you configured)', $res->status),
                'unexpected_redirect',
                $res->status,
                $requestId,
            );
        }
        try {
            $env = $raw === '' ? null : Json::decode($raw);
        } catch (\Throwable) {
            // not valid JSON as a whole (cut at the read limit, or not JSON at all): take the members that are complete at the
            // start, which is where the error envelope is; an HTML page or an empty body gives nothing and the status decides
            $env = Json::objectPrefix($raw);
        }
        $e = is_array($env) && is_array($env['error'] ?? null) ? $env['error'] : [];
        // What the server sends is data, not a log line: control characters, bidi overrides and the like become spaces and
        // every field is bounded, so a hostile or broken answer cannot forge log lines or flood a message.
        $code = is_string($e['code'] ?? null) ? self::clean($e['code'], 64) : '';
        $code = $code !== '' ? $code : self::fallbackCode($res->status);
        $message = is_string($e['message'] ?? null) ? self::clean($e['message'], 500) : '';
        $message = $message !== '' ? $message : 'request failed with status ' . $res->status;
        if ($res->status === 404 && str_contains(strtolower($message), 'no such route')) {
            $message .= ' (check the API address you configured: it should be the address only, for example https://host; a trailing /v1 is accepted)';
        }
        if (is_string($e['requestId'] ?? null) && self::clean($e['requestId'], 128) !== '') {
            $requestId = self::clean($e['requestId'], 128);
        }
        /** @var array<string, mixed>|null $details */
        $details = is_array($e['details'] ?? null) ? $e['details'] : null;
        $header = $res->header('retry-after');
        $retryAfter = $header === null ? null : self::parseRetryAfter($header)['seconds'];

        throw VunlyError::fromStatus($res->status, $code, $message, $requestId, $details, $retryAfter);
    }

    /**
     * How a Retry-After header is read: the same rule in every SDK (conformance/README.md, vectors in
     * conformance/retry-after-vectors.json). A plain decimal number of seconds up to 30 is the wait; a larger or unrepresentable
     * number is a very long wait (`surface`, capped at 86,400), never an absent header; anything else is `ignored`.
     *
     * @return array{kind: 'seconds'|'surface'|'ignored', seconds: float|null}
     *
     * @internal
     */
    public static function parseRetryAfter(string $header): array
    {
        $s = trim($header, " \t");
        if (preg_match('/^\+?(?:inf|infinity)$/Di', $s) === 1) {
            return ['kind' => 'surface', 'seconds' => Client::MAX_REPORTED_RETRY_AFTER_SECONDS];
        }
        // ASCII digits only (no /u): a non-ASCII digit, a hex form, a sign other than +, a suffix or a date is not a number
        if (preg_match('/^\+?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/D', $s) !== 1) {
            return ['kind' => 'ignored', 'seconds' => null];
        }
        $v = (float) $s; // too large to represent (1e999, 400 digits) is INF in PHP
        if (is_infinite($v) || $v > Client::MAX_RETRY_AFTER_SECONDS) {
            return ['kind' => 'surface', 'seconds' => min($v, Client::MAX_REPORTED_RETRY_AFTER_SECONDS)];
        }

        return ['kind' => 'seconds', 'seconds' => $v];
    }

    /**
     * Makes a server-supplied string safe to put in a message or a log line: control characters, DEL, C1 controls, bidi
     * overrides and isolates and zero-width marks become spaces, runs of spaces collapse, and the result is cut to $max
     * characters. Bytes that are not valid UTF-8 become spaces too.
     *
     * @internal
     */
    public static function clean(string $s, int $max): string
    {
        $out = preg_replace('/[\x{0000}-\x{001F}\x{007F}-\x{009F}\x{061C}\x{200B}-\x{200F}\x{2028}-\x{202E}\x{2060}-\x{2069}\x{FEFF}]/u', ' ', $s);
        if ($out === null) {
            $out = (string) preg_replace('/[^\x20-\x7E]/', ' ', $s); // not UTF-8: keep printable ASCII only
        }
        $out = trim((string) preg_replace('/ {2,}/', ' ', $out));
        if (preg_match('/^.{0,' . $max . '}/su', $out, $m) === 1) {
            $out = rtrim($m[0]);
        }

        return $out;
    }

    private static function fallbackCode(int $status): string
    {
        return match (true) {
            $status === 401, $status === 403 => 'unauthenticated',
            $status === 402 => 'insufficient_credits',
            $status === 429 => 'rate_limited',
            $status >= 500 => 'internal_error',
            default => 'validation_error',
        };
    }

    /** Unavailable whatever the status: the call may have been applied, so retrying later with the SAME id is safe. */
    private static function invalidResponse(int $status, ?string $requestId, string $what): Unavailable
    {
        return new Unavailable(
            sprintf('the API answered %d but the response is malformed (%s)', $status, $what),
            'invalid_response',
            $status,
            $requestId,
        );
    }

    /**
     * @param array<string, mixed>  $body
     * @param array<string, string> $shape
     */
    private static function shapeProblem(array $body, array $shape): ?string
    {
        foreach ($shape as $path => $spec) {
            $nullable = str_ends_with($spec, '?');
            $type = rtrim($spec, '?');
            $v = $body;
            foreach (explode('.', $path) as $part) {
                $v = is_array($v) ? ($v[$part] ?? null) : null;
            }
            if ($v === null) {
                if ($nullable) {
                    continue;
                }

                return "field \"$path\" is missing";
            }
            $good = match ($type) {
                'string' => is_string($v),
                'bool' => is_bool($v),
                'int' => is_int($v),
                'array' => is_array($v) && array_is_list($v),
                'object' => is_array($v) && ($v === [] || !array_is_list($v)),
                default => false,
            };
            if (!$good) {
                return "field \"$path\" is not $type";
            }
        }

        return null;
    }
}
