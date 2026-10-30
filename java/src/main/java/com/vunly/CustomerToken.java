package com.vunly;

import java.time.Instant;

/**
 * A short-lived read token for ONE customer, for the usage widget in a browser. It is shown once; a browser may hold
 * it, never an API key. {@link #toString()} does not print the token.
 */
public record CustomerToken(String id, String token, String customerId, Instant expiresAt) {
  static CustomerToken from(Fields f) {
    return new CustomerToken(f.str("id"), f.str("token"), f.str("customerId"), f.instant("expiresAt"));
  }

  @Override
  public String toString() {
    return "CustomerToken[id=" + id + ", customerId=" + customerId + ", expiresAt=" + expiresAt + ", token=<hidden>]";
  }
}
