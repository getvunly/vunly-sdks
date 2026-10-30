package com.vunly;

import java.time.Duration;
import java.util.LinkedHashMap;
import java.util.Map;

/** Short-lived read tokens for one customer, for a browser. */
public final class CustomerTokensApi {
  private final Transport t;

  CustomerTokensApi(Transport t) {
    this.t = t;
  }

  /** As {@link #create(String, Duration)} with the default lifetime (15 minutes). */
  public CustomerToken create(String customer) {
    return create(customer, null);
  }

  /**
   * Mints a token for ONE customer. {@code ttl} is 30 seconds to 1 hour (null means 15 minutes). Every call mints a new
   * token and has no idempotency key, so after a lost answer the SDK does NOT repeat it: it throws an
   * {@link UnavailableException} and you decide.
   */
  public CustomerToken create(String customer, Duration ttl) {
    Map<String, Object> body = new LinkedHashMap<>();
    body.put("customerId", customer);
    if (ttl != null) {
      body.put("ttlSeconds", ttl.toSeconds());
    }
    return t.call(
            new Transport.Request(
                "POST", "/v1/customer-tokens", null, body, null, Transport.OpClass.X),
            CustomerToken::from)
        .value();
  }

  /** Revokes a token by its id. Idempotent. */
  public void revoke(String id) {
    t.call(
        new Transport.Request(
            "DELETE", "/v1/customer-tokens/" + Transport.segment(id), null, null, null, Transport.OpClass.N),
        null);
  }
}
