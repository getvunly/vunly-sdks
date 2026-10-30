package com.vunly;

import java.util.LinkedHashMap;
import java.util.Map;

/** Asks whether a customer may do something now. */
public final class EntitlementsApi {
  private final Transport t;

  EntitlementsApi(Transport t) {
    this.t = t;
  }

  /** As {@link #check(String, String, Estimate)} with no estimate. */
  public Entitlement check(String customer, String feature) {
    return check(customer, feature, null);
  }

  /**
   * Read-only and advisory. If it throws (an {@link UnavailableException} after retries), treat that as "no": fail
   * closed, do not run the work. It never returns {@code allowed == true} by itself.
   *
   * @param estimate optional: the usage the work is expected to need
   */
  public Entitlement check(String customer, String feature, Estimate estimate) {
    Map<String, Object> body = new LinkedHashMap<>();
    body.put("customerId", customer);
    body.put("feature", feature);
    if (estimate != null) {
      body.put("estimate", Map.of("meter", estimate.meter(), "quantity", estimate.quantity()));
    }
    return t.call(
            new Transport.Request(
                "POST", "/v1/entitlements/check", null, body, null, Transport.OpClass.R),
            Entitlement::from)
        .value();
  }
}
