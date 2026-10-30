<?php

declare(strict_types=1);

namespace Vunly\Http;

/**
 * One HTTP attempt, as the SDK hands it to a {@see Transport}. The headers include the Authorization header, so the
 * object prints as nothing but its method and path.
 */
final class Request
{
    /**
     * @param array<string, string> $headers
     */
    public function __construct(
        public readonly string $method,
        public readonly string $url,
        #[\SensitiveParameter]
        public readonly array $headers,
        public readonly ?string $body,
        /** Seconds for the WHOLE attempt: connect, headers and body together. */
        public readonly float $timeoutSeconds,
        /** Largest response body to accept, in bytes. A transport stops reading at this size. */
        public readonly int $maxBodyBytes,
        /** Largest body to accept for a non-2xx answer, in bytes: only its status and a short error matter. */
        public readonly int $maxErrorBodyBytes = 65_536,
    ) {
    }

    /** @return array<string, string> */
    public function __debugInfo(): array
    {
        return ['method' => $this->method, 'path' => (string) parse_url($this->url, PHP_URL_PATH)];
    }
}
