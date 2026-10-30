package com.vunly;

/**
 * The answer of {@link EntitlementsApi#check}. Advisory: it reserves nothing, the authoritative limit is the debit made
 * by {@link UsageApi#record}. Unless {@code allowed} is true, do not run the work.
 *
 * @param reason "ok", "no_active_plan", "feature_not_in_plan" or "insufficient_credits"
 * @param plan null when the customer has none
 */
public record Entitlement(boolean allowed, String reason, long remaining, long required, String plan) {
  static Entitlement from(Fields f) {
    return new Entitlement(
        f.bool("allowed"), f.str("reason"), f.lng("remaining"), f.lng("required"), f.strOrNull("plan"));
  }
}
