<?php

declare(strict_types=1);

namespace Vunly;

/** A 401 or 403: a missing, wrong or revoked key, or a key without the scope. Not retryable. */
final class AuthError extends VunlyError
{
}
