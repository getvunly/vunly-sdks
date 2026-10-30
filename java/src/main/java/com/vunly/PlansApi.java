package com.vunly;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Meters, plans and subscriptions. */
public final class PlansApi {
  private final Transport t;

  PlansApi(Transport t) {
    this.t = t;
  }

  /** The fields of {@link #upsertPlan}. */
  public record PlanSpec(String key, String name, long includedCredits, String meter, List<String> features) {}

  /** Creates or updates a meter: how many units of usage cost one credit. */
  public void upsertMeter(String key, long unitsPerCredit) {
    Map<String, Object> body = new LinkedHashMap<>();
    body.put("key", key);
    body.put("unitsPerCredit", unitsPerCredit);
    t.call(new Transport.Request("POST", "/v1/meters", null, body, null, Transport.OpClass.N), null);
  }

  /** Creates or updates a plan. */
  public Plan upsertPlan(PlanSpec spec) {
    Map<String, Object> body = new LinkedHashMap<>();
    body.put("key", spec.key());
    body.put("name", spec.name());
    body.put("includedCredits", spec.includedCredits());
    body.put("meter", spec.meter());
    body.put("features", spec.features() == null ? List.of() : spec.features());
    return t.call(
            new Transport.Request("POST", "/v1/plans", null, body, null, Transport.OpClass.N),
            Plan::strict)
        .value();
  }

  /**
   * Activates a plan for the customer. The same plan again returns the existing subscription and grants nothing more;
   * another plan while one is active is an {@link InvalidRequestException} with code "subscription_exists".
   */
  public Subscription subscribe(String customer, String plan) {
    return t.call(
            new Transport.Request(
                "POST",
                "/v1/customers/" + Transport.segment(customer) + "/subscription",
                null,
                Map.of("plan", plan),
                null,
                Transport.OpClass.N),
            Subscription::from)
        .value();
  }

  /** Cancels the customer's active subscription. */
  public void cancel(String customer) {
    t.call(
        new Transport.Request(
            "DELETE",
            "/v1/customers/" + Transport.segment(customer) + "/subscription",
            null,
            null,
            null,
            Transport.OpClass.N),
        null);
  }
}
