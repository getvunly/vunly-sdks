package com.vunly;

import java.time.Instant;

/**
 * The project's plan and this month's recorded usage.
 *
 * <p>{@link Limits} is a working hypothesis, not approved and not a guarantee: {@code confirmed} is false. Do not show
 * it to your customers as a quota, a price or a commitment.
 */
public record PlanUsage(PlanRef plan, Instant periodStart, Instant periodEnd, long events, Limits unconfirmedLimits) {

  /** Names a plan. */
  public record PlanRef(String key, String name) {}

  /**
   * @param includedMonthlyEvents null when none is proposed
   * @param activityRetentionDays null when none is proposed
   */
  public record Limits(boolean confirmed, String note, Long includedMonthlyEvents, Long activityRetentionDays) {}

  static PlanUsage from(Fields f) {
    Fields plan = f.obj("plan");
    Fields period = f.obj("period");
    Fields limits = f.obj("unconfirmedLimits");
    return new PlanUsage(
        new PlanRef(plan.str("key"), plan.strOrNull("name")),
        period.instant("start"),
        period.instant("end"),
        f.obj("usage").lng("events"),
        new Limits(
            limits.bool("confirmed"),
            limits.strOrNull("note"),
            limits.lngOrNull("includedMonthlyEvents"),
            limits.lngOrNull("activityRetentionDays")));
  }
}
