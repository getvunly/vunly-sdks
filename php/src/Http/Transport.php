<?php

declare(strict_types=1);

namespace Vunly\Http;

/**
 * Sends ONE attempt. The SDK owns retries, so an implementation MUST NOT retry, replay or follow redirects, MUST bound
 * the whole attempt by the request's timeout, and MUST stop reading at the body cap. It never throws for a network
 * problem: it returns a {@see Response} with a {@see Failure}.
 */
interface Transport
{
    public function send(Request $request): Response;
}
