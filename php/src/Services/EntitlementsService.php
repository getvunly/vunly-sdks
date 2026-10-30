<?php

declare(strict_types=1);

namespace Vunly\Services;

use Vunly\Client;
use Vunly\Executor;
use Vunly\Models\Entitlement;
use Vunly\Models\Estimate;

/** Asks whether a customer may do something now. */
final class EntitlementsService
{
    /** @internal */
    public function __construct(private readonly Executor $client)
    {
    }

    /**
     * Read-only and advisory. If it throws (an Unavailable after retries), treat that as "no": fail closed, do not run
     * the work. It never returns `allowed === true` by itself.
     */
    public function check(mixed $customer, string $feature, ?Estimate $estimate = null): Entitlement
    {
        Client::segment($customer, 'customer'); // refuse null, a non-string, "", "." and "..", as the path-based calls do
        \assert(is_string($customer));
        $body = ['customerId' => $customer, 'feature' => $feature];
        if ($estimate !== null) {
            $body['estimate'] = ['meter' => $estimate->meter, 'quantity' => $estimate->quantity];
        }

        return $this->client->fetch(
            'POST',
            '/v1/entitlements/check',
            Entitlement::fromArray(...),
            ['allowed' => 'bool', 'reason' => 'string', 'remaining' => 'int', 'required' => 'int', 'plan' => 'string?'],
            body: $body,
        )->value;
    }
}
