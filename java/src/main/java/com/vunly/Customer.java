package com.vunly;

import java.time.Instant;

/**
 * A customer you bill. {@code id} is Vunly's own id (cus_...); you never have to store it: every call accepts your
 * {@code externalId} instead.
 *
 * @param name null when none was given
 */
public record Customer(String id, String externalId, String name, Instant createdAt) {
  static Customer from(Fields f) {
    return new Customer(f.str("id"), f.str("externalId"), f.strOrNull("name"), f.instant("createdAt"));
  }
}
