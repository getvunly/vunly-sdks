package com.vunly;

import java.time.Instant;
import java.util.List;

/**
 * A plan of the project. When it comes from a {@link Subscription} only {@code key} is guaranteed; the other fields are
 * null, 0 or empty if the API did not send them.
 */
public record Plan(
    String key, String name, long includedCredits, String meter, List<String> features, Instant updatedAt) {
  static Plan from(Fields f) {
    return new Plan(
        f.str("key"),
        f.strOrNull("name"),
        f.lngOr("includedCredits", 0),
        f.strOrNull("meter"),
        f.has("features") ? f.strings("features") : List.of(),
        f.instantOrNull("updatedAt"));
  }

  /** The full shape {@link PlansApi#upsertPlan} promises. */
  static Plan strict(Fields f) {
    f.str("name");
    f.lng("includedCredits");
    f.str("meter");
    f.strings("features");
    return from(f);
  }
}
