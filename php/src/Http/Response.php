<?php

declare(strict_types=1);

namespace Vunly\Http;

/** What a {@see Transport} returns. `failure` is set when the attempt did not end in a complete answer. */
final class Response
{
    /**
     * @param array<string, string> $headers lower-case names; repeated headers are joined with ", "
     */
    public function __construct(
        public readonly int $status,
        public readonly array $headers,
        public readonly string $body,
        public readonly ?Failure $failure = null,
    ) {
    }

    /** @param array<string, string> $headers */
    public static function failed(Failure $failure, int $status = 0, array $headers = []): self
    {
        return new self($status, $headers, '', $failure);
    }

    public function header(string $name): ?string
    {
        return $this->headers[strtolower($name)] ?? null;
    }
}
