package com.vunly;

import java.util.LinkedHashMap;
import java.util.Map;

/** Records usage events. */
public final class UsageApi {
  private final Transport t;

  UsageApi(Transport t) {
    this.t = t;
  }

  /**
   * Records usage once and debits credits. It is final: a {@link LimitReachedException} means the credits were not there
   * when you recorded, even if a check said yes earlier; nothing was debited. A repeat of the same id returns the
   * original result with {@code replayed == true}.
   *
   * @param id names the unit of work (your request or job id) and is sent as the Idempotency-Key: 8 to 128 characters
   *     of {@code A-Za-z0-9_.:-}. Retry with the SAME id; the API applies it at most once. It is never generated for
   *     you.
   * @param quantity in the meter's own units (for example tokens), not credits: the project's meter converts it,
   *     rounded up per request
   */
  public UsageResult record(String id, String customer, String meter, long quantity) {
    Map<String, Object> body = new LinkedHashMap<>();
    body.put("customerId", customer);
    body.put("meter", meter);
    body.put("quantity", quantity);
    Transport.Result<Object[]> r =
        t.call(
            new Transport.Request("POST", "/v1/usage-events", null, body, id, Transport.OpClass.K),
            f -> new Object[] {LedgerEntry.from(f.obj("event")), f.lng("balance")});
    Transport.Meta m = r.meta();
    return new UsageResult(
        (LedgerEntry) r.value()[0],
        (Long) r.value()[1],
        m.replayed(),
        m.planOverage() && !m.replayed(),
        m.requestId());
  }
}
