<?php

declare(strict_types=1);

namespace Vunly;

/**
 * Base class of every failure of an API call. Catch one of the four typed subclasses
 * ({@see AuthError}, {@see LimitReached}, {@see InvalidRequest}, {@see Unavailable}), never match on the message.
 *
 * The message never contains the API key, a customer token, a header or the address that was called.
 * getCode() is the HTTP status (0 when there was no response).
 */
abstract class VunlyError extends \RuntimeException
{
    /**
     * @param array<string, mixed>|null $details the API's structured detail (for example balance and required on a 402)
     */
    final public function __construct(
        string $message,
        /** The API's stable machine code ("insufficient_credits", ...) or an SDK code ("unavailable", "invalid_response"). */
        public readonly string $errorCode,
        /** HTTP status, or 0 when there was no response. */
        public readonly int $status,
        /** The API's Request-Id. Quote it in a support request. */
        public readonly ?string $requestId = null,
        public readonly ?array $details = null,
        /** The server's Retry-After in seconds, when it sent one. */
        public readonly ?float $retryAfterSeconds = null,
    ) {
        parent::__construct($message, $status);
    }

    /** Whether trying again later (with the SAME id) can help. */
    public function isRetryable(): bool
    {
        return false;
    }

    /**
     * Builds the class the status belongs to: 402 LimitReached, 401/403 AuthError, 429/5xx/0 Unavailable, every other
     * status InvalidRequest.
     *
     * @param array<string, mixed>|null $details
     *
     * @internal
     */
    public static function fromStatus(
        int $status,
        string $code,
        string $message,
        ?string $requestId = null,
        ?array $details = null,
        ?float $retryAfterSeconds = null,
    ): self {
        $class = match (true) {
            $status === 402 => LimitReached::class,
            $status === 401, $status === 403 => AuthError::class,
            $status === 429, $status >= 500, $status === 0 => Unavailable::class,
            default => InvalidRequest::class,
        };

        return new $class($message, $code, $status, $requestId, $details, $retryAfterSeconds);
    }

    /** @internal */
    public function __toString(): string
    {
        // The default Throwable string embeds the stack trace with call arguments; keep it to what is safe to log.
        return static::class . ': ' . $this->getMessage()
            . ($this->status !== 0 ? " ({$this->status} {$this->errorCode})" : " ({$this->errorCode})")
            . ($this->requestId !== null ? " request {$this->requestId}" : '');
    }
}
