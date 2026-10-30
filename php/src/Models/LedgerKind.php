<?php

declare(strict_types=1);

namespace Vunly\Models;

/**
 * The kinds of ledger entry this SDK version knows. The API may add kinds: {@see LedgerEntry::$kind} is always the raw
 * string, and {@see LedgerEntry::knownKind()} returns null for one this enum does not list.
 */
enum LedgerKind: string
{
    case Grant = 'grant';
    case Debit = 'debit';
}
