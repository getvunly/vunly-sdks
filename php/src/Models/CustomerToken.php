<?php

declare(strict_types=1);

namespace Vunly\Models;

use Vunly\Secret;

/**
 * A short-lived read token for ONE customer, for the usage widget in a browser. It is shown once; a browser may hold it,
 * never an API key. Printing, dumping, serializing or JSON-encoding the object shows only its id: read the value with
 * `$token->token->reveal()` at the one place that hands it to the browser.
 */
final readonly class CustomerToken implements \JsonSerializable
{
    public function __construct(
        public string $id,
        public Secret $token,
        public string $customerId,
        public \DateTimeImmutable $expiresAt,
    ) {
    }

    /**
     * @param array<string, mixed> $d
     *
     * @internal
     */
    public static function fromArray(array $d): self
    {
        return new self(
            Fields::string($d, 'id'),
            new Secret(Fields::string($d, 'token')),
            Fields::string($d, 'customerId'),
            Fields::date(Fields::string($d, 'expiresAt')),
        );
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return [
            'id' => $this->id,
            'token' => Secret::PLACEHOLDER,
            'customerId' => $this->customerId,
            'expiresAt' => Fields::iso($this->expiresAt),
        ];
    }

    /** @return array<string, mixed> */
    public function __debugInfo(): array
    {
        return $this->jsonSerialize();
    }
}
