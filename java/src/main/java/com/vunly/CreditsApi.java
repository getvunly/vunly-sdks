package com.vunly;

import java.util.LinkedHashMap;
import java.util.Map;

/** Grants credits and reads balances and the ledger. */
public final class CreditsApi {
  private final Transport t;

  CreditsApi(Transport t) {
    this.t = t;
  }

  /** As {@link #grant(String, String, long, String)} with no reason. */
  public LedgerEntry grant(String id, String customer, long credits) {
    return grant(id, customer, credits, null);
  }

  /**
   * Adds credits (a sandbox top-up). {@code id} is the Idempotency-Key (8 to 128 characters of {@code A-Za-z0-9_.:-}):
   * repeating the same id grants once and returns the original entry.
   */
  public LedgerEntry grant(String id, String customer, long credits, String reason) {
    Map<String, Object> body = new LinkedHashMap<>();
    body.put("credits", credits);
    if (reason != null && !reason.isEmpty()) {
      body.put("reason", reason);
    }
    return t.call(
            new Transport.Request(
                "POST",
                "/v1/customers/" + Transport.segment(customer) + "/credit-grants",
                null,
                body,
                id,
                Transport.OpClass.K),
            LedgerEntry::from)
        .value();
  }

  /** A customer's credits. */
  public Balance balance(String customer) {
    return t.call(
            Transport.Request.get("/v1/customers/" + Transport.segment(customer) + "/balance", null),
            Balance::from)
        .value();
  }

  /** One page of a customer's ledger entries, newest first. */
  public Page<LedgerEntry> ledger(String customer, ListOptions options) {
    return t.call(
            Transport.Request.get(
                "/v1/customers/" + Transport.segment(customer) + "/ledger", Transport.query(options)),
            f -> CustomersApi.page(f, LedgerEntry::from))
        .value();
  }

  /** A customer's whole ledger, one page at a time, only as you consume it. */
  public Iterable<LedgerEntry> ledgerAll(String customer, int pageSize) {
    return Pages.iterable(cursor -> ledger(customer, new ListOptions(pageSize, cursor)));
  }
}
