<?php

declare(strict_types=1);

namespace Vunly;

/**
 * Every other 4xx (400, 404, 409) and every local validation failure (for example an idempotency id that could not be
 * an Idempotency-Key, refused before any request). Fix the request. Not retryable.
 */
final class InvalidRequest extends VunlyError
{
}
