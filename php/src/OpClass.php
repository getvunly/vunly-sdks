<?php

declare(strict_types=1);

namespace Vunly;

/**
 * The operation classes of conformance/README.md: what may be repeated after a failure.
 *
 * @internal
 */
enum OpClass: string
{
    /** Reads and the entitlement check. */
    case Read = 'R';
    /** Keyed mutations: repeated with the SAME Idempotency-Key. */
    case Keyed = 'K';
    /** Naturally idempotent writes: upserts, deletes. */
    case Natural = 'N';
    /** Everything else (a token mint): never repeated after an ambiguous failure, only after a 429. */
    case Other = 'X';
}
