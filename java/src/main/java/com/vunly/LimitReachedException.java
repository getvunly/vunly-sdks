package com.vunly;

import java.time.Duration;
import java.util.Map;
import java.util.OptionalLong;

/** 402: the customer has too few credits. Nothing was debited. Do not run the work. Not retryable. */
public final class LimitReachedException extends VunlyException {
  private static final long serialVersionUID = 1L;

  LimitReachedException(
      String code, int status, String message, String requestId, Map<String, Object> details, Duration retryAfter) {
    super(code, status, message, requestId, details, retryAfter);
  }

  /** The customer's balance reported by the 402, when the API sent it. */
  public OptionalLong balance() {
    return number("balance");
  }

  /** The cost the refused request needed, when the API sent it. */
  public OptionalLong required() {
    return number("required");
  }

  private OptionalLong number(String key) {
    Object v = details().get(key);
    return v instanceof Number n ? OptionalLong.of(n.longValue()) : OptionalLong.empty();
  }
}
