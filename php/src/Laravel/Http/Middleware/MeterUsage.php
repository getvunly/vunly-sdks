<?php

declare(strict_types=1);

namespace Vunly\Laravel\Http\Middleware;

use Closure;
use Illuminate\Contracts\Auth\Authenticatable;
use Illuminate\Contracts\Config\Repository;
use Illuminate\Http\JsonResponse;
use Illuminate\Http\Request;
use Symfony\Component\HttpFoundation\Response;
use Vunly\Client;
use Vunly\LimitReached;
use Vunly\Models\Estimate;
use Vunly\Unavailable;
use Vunly\VunlyError;

/**
 * Route middleware `vunly:<feature>,<meter>[,<estimate>]`: CHECK before the work, RECORD after it.
 *
 *     Route::post('/generate', GenerateController::class)->middleware('vunly:generate,tokens,1000');
 *
 *  1. Before the controller it asks Vunly whether the customer may do `feature`, pricing in `estimate` units of `meter`.
 *     "No", and any failure of the check, mean the controller does not run (fail closed): 403 for a "no", 503 when
 *     Vunly could not be reached.
 *  2. After the controller, if the response is not an error (status below 400), it records the usage once, under an id
 *     made on the server, ONCE per request (the SDK retries inside the request with that same id). The caller's
 *     X-Request-Id is IGNORED by default: honouring it would let anyone repeat a header and get the work for free (the API
 *     answers "replayed" and charges nothing). `vunly.trust_request_id` turns it on, bound to customer, feature and meter.
 *  3. The quantity recorded is the `vunly.quantity` request attribute when the controller set it
 *     (`$request->attributes->set('vunly.quantity', 1050)`), otherwise the estimate.
 *
 * If recording finds no credits (402) the response is replaced with a 402; if Vunly cannot be reached the response is
 * replaced with a 503 (its body carries the request id used). The remaining credits and whether the id was
 * a replay come back as the headers `X-Vunly-Credits-Remaining` and `X-Vunly-Replayed`.
 *
 * Who is the customer? By default the authenticated user's id; change it once, at boot, with
 * `MeterUsage::resolveCustomerUsing(fn (Request $r): ?string => ...)`. A customer that cannot be resolved gets a 401.
 */
final class MeterUsage
{
    /** @var (Closure(Request): (string|int|null))|null */
    private static ?Closure $customerResolver = null;

    public function __construct(private readonly Client $vunly, private readonly Repository $config)
    {
    }

    /** @param (Closure(Request): (string|int|null))|null $resolver null restores the default (the authenticated user's id) */
    public static function resolveCustomerUsing(?Closure $resolver): void
    {
        self::$customerResolver = $resolver;
    }

    public function handle(Request $request, Closure $next, string $feature, string $meter, string $estimate = '1'): Response
    {
        if (preg_match('/^[1-9][0-9]{0,15}$/D', $estimate) !== 1) {
            throw new \InvalidArgumentException('Vunly: the middleware estimate must be a positive integer, e.g. "vunly:generate,tokens,1000".');
        }
        $units = (int) $estimate;

        $user = $request->user();
        $resolved = self::$customerResolver !== null
            ? (self::$customerResolver)($request)
            : ($user instanceof Authenticatable ? $user->getAuthIdentifier() : null);
        $customer = is_int($resolved) ? (string) $resolved : $resolved;
        if (!is_string($customer) || $customer === '') {
            return self::json(['error' => 'customer_unresolved'], 401);
        }

        $id = 'req_' . bin2hex(random_bytes(16));
        if ($this->config->get('vunly.trust_request_id') === true) {
            // Opt-in. The client's header names the unit of work, so repeating it is a replay: the work runs again and is NOT
            // charged again. The id is therefore bound to the customer, the feature and the meter (another customer, or another
            // route, cannot collide with it), but there is no time window: see the README before enabling this.
            $header = $request->headers->get('X-Request-Id');
            if (is_string($header) && preg_match('/^[A-Za-z0-9_.:-]{8,128}$/D', $header) === 1) {
                $id = 'req_' . substr(hash('sha256', implode("\0", [$customer, $feature, $meter, $header])), 0, 40);
            }
        }
        $request->attributes->set('vunly.customer', $customer);
        $request->attributes->set('vunly.request_id', $id);

        // 1. ask first; an error is a "no"
        try {
            $decision = $this->vunly->entitlements->check($customer, $feature, new Estimate($meter, $units));
        } catch (VunlyError $e) {
            return self::failure($e, $id);
        }
        if (!$decision->allowed) {
            return self::json(['error' => 'not_allowed', 'reason' => $decision->reason], 403);
        }

        // 2. the work (an exception here records nothing)
        $response = $next($request);
        if ($response->getStatusCode() >= 400) {
            return $response;
        }

        // 3. record it once
        $quantity = $request->attributes->get('vunly.quantity');
        $quantity = is_int($quantity) && $quantity > 0 ? $quantity : $units;
        try {
            $result = $this->vunly->usage->record($id, $customer, $meter, $quantity);
        } catch (VunlyError $e) {
            return self::failure($e, $id);
        }
        $response->headers->set('X-Vunly-Credits-Remaining', (string) $result->remaining);
        $response->headers->set('X-Vunly-Replayed', $result->replayed ? 'true' : 'false');

        return $response;
    }

    private static function failure(VunlyError $e, string $id): JsonResponse
    {
        if ($e instanceof LimitReached) {
            return self::json(['error' => 'insufficient_credits', 'requestId' => $e->requestId], 402);
        }
        if ($e instanceof Unavailable) {
            // fail closed. The outcome of the record is unknown; the id is returned so it can be looked up. Repeating the request
            // makes a new id (the caller's header is ignored unless trust_request_id is on), so it may be billed again.
            return self::json(['error' => 'billing_unavailable', 'code' => $e->errorCode, 'requestId' => $id], 503);
        }

        // a key the API refuses, or a request it rejects, is a problem with this application's setup, not the caller's
        return self::json(['error' => 'billing_misconfigured', 'code' => $e->errorCode], 500);
    }

    /** @param array<string, mixed> $body */
    private static function json(array $body, int $status): JsonResponse
    {
        return new JsonResponse($body, $status);
    }
}
