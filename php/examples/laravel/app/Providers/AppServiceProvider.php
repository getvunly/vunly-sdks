<?php

declare(strict_types=1);

namespace App\Providers;

use Illuminate\Http\Request;
use Illuminate\Support\ServiceProvider;
use Vunly\Laravel\Http\Middleware\MeterUsage;

final class AppServiceProvider extends ServiceProvider
{
    public function boot(): void
    {
        // Who is the billed customer? A real application answers with its authenticated user or tenant:
        //     MeterUsage::resolveCustomerUsing(fn (Request $r) => $r->user()?->getAuthIdentifier());
        // This example has no sign-in, so it takes the customer from the request body. NEVER do that in a real
        // application: anyone could then spend someone else's credits.
        MeterUsage::resolveCustomerUsing(static function (Request $request): ?string {
            $customer = $request->input('customer');

            return is_string($customer) ? $customer : null;
        });
    }
}
