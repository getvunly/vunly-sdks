package com.vunly;

import java.time.Instant;

/**
 * A customer's plan.
 *
 * @param status "active" or "canceled"
 */
public record Subscription(
    String id,
    String customerId,
    Plan plan,
    String status,
    Instant periodStart,
    Instant periodEnd,
    Instant createdAt) {
  static Subscription from(Fields f) {
    return new Subscription(
        f.str("id"),
        f.str("customerId"),
        Plan.from(f.obj("plan")),
        f.str("status"),
        f.instantOrNull("periodStart"),
        f.instantOrNull("periodEnd"),
        f.instantOrNull("createdAt"));
  }
}
