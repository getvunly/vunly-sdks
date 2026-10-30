package com.vunly;

import java.time.Instant;

/**
 * One line of a customer's ledger.
 *
 * @param kind "grant" or "debit"
 * @param credits signed: positive for a grant, negative for a debit
 * @param meter null for a grant
 * @param quantity 0 for a grant
 * @param reason null when none was given
 */
public record LedgerEntry(
    String id,
    String customerId,
    String kind,
    long credits,
    String meter,
    long quantity,
    String reason,
    String idempotencyKey,
    Instant createdAt) {
  static LedgerEntry from(Fields f) {
    return new LedgerEntry(
        f.str("id"),
        f.str("customerId"),
        f.str("kind"),
        f.lng("credits"),
        f.strOrNull("meter"),
        f.lngOr("quantity", 0),
        f.strOrNull("reason"),
        f.strOrNull("idempotencyKey"),
        f.instant("createdAt"));
  }
}
