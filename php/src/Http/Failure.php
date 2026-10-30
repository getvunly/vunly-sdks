<?php

declare(strict_types=1);

namespace Vunly\Http;

/** Why an attempt produced no complete response. The underlying library's own error text is deliberately dropped. */
enum Failure: string
{
    /** The whole attempt did not finish within the timeout (connecting, waiting, or a body that kept dripping). */
    case Timeout = 'timeout';
    /** The connection could not be made or was cut before the answer was complete. */
    case Connection = 'connection';
    /** The body was larger than the cap; reading stopped. The status and headers are still known. */
    case TooLarge = 'too_large';
}
