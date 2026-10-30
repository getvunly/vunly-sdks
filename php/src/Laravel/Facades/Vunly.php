<?php

declare(strict_types=1);

namespace Vunly\Laravel\Facades;

use Illuminate\Support\Facades\Facade;
use Vunly\Client;

/**
 * Static access to the shared {@see Client}. A facade forwards method calls only, so use the service methods:
 * `Vunly::customers()->upsert('acme')`, `Vunly::usage()->record(...)`, `Vunly::planUsage()`.
 *
 * @method static \Vunly\Services\CustomersService      customers()
 * @method static \Vunly\Services\CreditsService        credits()
 * @method static \Vunly\Services\UsageService          usage()
 * @method static \Vunly\Services\EntitlementsService   entitlements()
 * @method static \Vunly\Services\PlansService          plans()
 * @method static \Vunly\Services\CustomerTokensService customerTokens()
 * @method static \Vunly\Models\PlanUsage               planUsage()
 *
 * @see Client
 */
final class Vunly extends Facade
{
    protected static function getFacadeAccessor(): string
    {
        return Client::class;
    }
}
