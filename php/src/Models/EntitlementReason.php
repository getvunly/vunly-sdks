<?php

declare(strict_types=1);

namespace Vunly\Models;

/**
 * The reasons of an entitlement answer this SDK version knows. The API may add reasons:
 * {@see Entitlement::$reason} is always the raw string.
 */
enum EntitlementReason: string
{
    case Ok = 'ok';
    case NoActivePlan = 'no_active_plan';
    case FeatureNotInPlan = 'feature_not_in_plan';
    case InsufficientCredits = 'insufficient_credits';
}
