<?php

declare(strict_types=1);

namespace Vunly;

/** Verifies the signature of a webhook delivery. No network, no client needed. */
final class Webhooks
{
    /** How far a signed timestamp may be from now (either way), in seconds, before it counts as stale. */
    public const TOLERANCE_SECONDS = 300;

    private function __construct()
    {
    }

    /**
     * Checks a delivery. `$rawBody` is the RAW request body (the exact bytes, not re-encoded JSON), `$signatureHeader`
     * the Vunly-Signature header ("t=<unix>,v1=<hex>") and `$secrets` the endpoint's signing secret(s): pass two while
     * rotating, a match with any one is enough. The signature is HMAC-SHA256 over "<t>." + rawBody, compared in
     * constant time.
     *
     * @param string|list<string> $secrets
     * @param int|null            $now     unix seconds; defaults to the current time (tests)
     *
     * @throws WebhookError
     */
    public static function verify(
        string $rawBody,
        string $signatureHeader,
        #[\SensitiveParameter]
        string|array $secrets,
        ?int $now = null,
    ): WebhookEvent {
        // A missing setting (an unset WEBHOOK_SECRET read as '') must not make deliveries forgeable: HMAC with an empty key is
        // computable by anyone. This is a configuration error, raised before anything else, naming no value.
        $list = (array) $secrets;
        if ($list === []) {
            throw new \InvalidArgumentException('Vunly: no webhook signing secret was given (check where your endpoint secret is configured).');
        }
        foreach ($list as $secret) {
            // a usable secret has at least one printable ASCII character that is not a space (HMAC-SHA256, 300 s tolerance)
            if (!is_string($secret) || preg_match('/[\x21-\x7e]/', $secret) !== 1) {
                throw new \InvalidArgumentException('Vunly: a webhook signing secret is empty or not a string (check where your endpoint secret is configured).');
            }
        }
        if (trim($signatureHeader) === '') {
            throw new WebhookError('missing_header');
        }
        $timestamp = null;
        $signatures = [];
        foreach (explode(',', $signatureHeader) as $part) {
            $pair = explode('=', trim($part), 2);
            if (count($pair) !== 2) {
                continue;
            }
            if ($pair[0] === 't') {
                $timestamp = $pair[1];
            } elseif ($pair[0] === 'v1' && preg_match('/^[0-9a-f]{64}$/D', $pair[1]) === 1) {
                $signatures[] = $pair[1]; // a v1 that is not 64 hex characters is not a signature: the header is malformed
            }
        }
        if ($timestamp === null || preg_match('/^-?\d{1,15}$/D', $timestamp) !== 1 || $signatures === []) {
            throw new WebhookError('malformed_header');
        }
        if (abs(($now ?? time()) - (int) $timestamp) > self::TOLERANCE_SECONDS) {
            throw new WebhookError('stale_timestamp');
        }

        $matched = false;
        foreach ($list as $secret) {
            // HMAC over "<t>." + body, fed in pieces so the body is not copied into a second string (the receiver already
            // holds it once; a large delivery must not need twice its size to be refused)
            $ctx = hash_init('sha256', HASH_HMAC, $secret);
            hash_update($ctx, $timestamp);
            hash_update($ctx, '.');
            hash_update($ctx, $rawBody);
            $want = hash_final($ctx, true);
            foreach ($signatures as $got) {
                $bytes = ctype_xdigit($got) && strlen($got) % 2 === 0 ? hex2bin($got) : false;
                if ($bytes !== false && hash_equals($want, $bytes)) {
                    $matched = true;
                }
            }
        }
        if (!$matched) {
            throw new WebhookError('no_matching_signature');
        }

        try {
            $payload = Json::decode($rawBody);
            if (!is_array($payload)) {
                throw new \UnexpectedValueException('not an object');
            }
            /** @var array<string, mixed> $payload */

            return WebhookEvent::fromArray($payload);
        } catch (\Throwable) {
            throw new WebhookError('invalid_payload');
        }
    }
}
