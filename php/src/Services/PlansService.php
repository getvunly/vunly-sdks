<?php

declare(strict_types=1);

namespace Vunly\Services;

use Vunly\Client;
use Vunly\Executor;
use Vunly\Models\Plan;
use Vunly\Models\Subscription;
use Vunly\OpClass;

/** Manages meters, plans and subscriptions. */
final class PlansService
{
    /** @internal */
    public function __construct(private readonly Executor $client)
    {
    }

    /** Creates or updates a meter: how many units of usage cost one credit. */
    public function upsertMeter(string $key, int $unitsPerCredit): void
    {
        $this->client->call('POST', '/v1/meters', ['key' => $key, 'unitsPerCredit' => $unitsPerCredit]);
    }

    /**
     * Creates or updates a plan.
     *
     * @param list<string> $features
     */
    public function upsertPlan(string $key, string $name, int $includedCredits, string $meter, array $features = []): Plan
    {
        return $this->client->fetch(
            'POST',
            '/v1/plans',
            Plan::fromArray(...),
            ['key' => 'string', 'name' => 'string', 'includedCredits' => 'int', 'meter' => 'string', 'features' => 'array'],
            body: ['key' => $key, 'name' => $name, 'includedCredits' => $includedCredits, 'meter' => $meter, 'features' => array_values($features)],
            class: OpClass::Natural,
        )->value;
    }

    /**
     * Activates a plan for the customer. The same plan again returns the existing subscription and grants nothing more;
     * another plan while one is active is an InvalidRequest with code "subscription_exists".
     */
    public function subscribe(mixed $customer, string $plan): Subscription
    {
        return $this->client->fetch(
            'POST',
            '/v1/customers/' . Client::segment($customer, 'customer') . '/subscription',
            Subscription::fromArray(...),
            ['id' => 'string', 'customerId' => 'string', 'status' => 'string', 'plan.key' => 'string'],
            body: ['plan' => $plan],
            class: OpClass::Natural,
        )->value;
    }

    /** Cancels the customer's active subscription. */
    public function cancel(mixed $customer): void
    {
        $this->client->call('DELETE', '/v1/customers/' . Client::segment($customer, 'customer') . '/subscription');
    }
}
