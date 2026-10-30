<?php

declare(strict_types=1);

namespace Vunly\Services;

use Vunly\Client;
use Vunly\Executor;
use Vunly\Models\CustomerToken;
use Vunly\OpClass;

/** Mints short-lived read tokens for one customer, for a browser. */
final class CustomerTokensService
{
    /** @internal */
    public function __construct(private readonly Executor $client)
    {
    }

    /**
     * Mints a token for ONE customer. `ttlSeconds` is 30 to 3600 (null: 15 minutes). Every call mints a new token and has
     * no idempotency key, so after a lost answer the SDK does NOT repeat it: it throws Unavailable and you decide.
     */
    public function create(mixed $customer, ?int $ttlSeconds = null): CustomerToken
    {
        Client::segment($customer, 'customer'); // refuse null, a non-string, "", "." and "..", as the path-based calls do
        \assert(is_string($customer));
        $body = ['customerId' => $customer];
        if ($ttlSeconds !== null) {
            $body['ttlSeconds'] = $ttlSeconds;
        }

        return $this->client->fetch(
            'POST',
            '/v1/customer-tokens',
            CustomerToken::fromArray(...),
            ['id' => 'string', 'token' => 'string', 'customerId' => 'string', 'expiresAt' => 'string'],
            body: $body,
            class: OpClass::Other,
        )->value;
    }

    /** Revokes a token by its id. Idempotent. */
    public function revoke(mixed $id): void
    {
        $this->client->call('DELETE', '/v1/customer-tokens/' . Client::segment($id, 'id'));
    }
}
