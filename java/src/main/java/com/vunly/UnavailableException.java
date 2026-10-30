package com.vunly;

import java.time.Duration;
import java.util.Map;

/**
 * No answer (network, timeout), a 429, a 5xx that survived the SDK's own retries, a malformed answer
 * ({@code invalid_response}), or the calling thread being interrupted ({@code cancelled}). The outcome is unknown or
 * temporary: it is safe to try again later with the SAME id, or to fail closed. A 429 whose details say
 * {@code reason=plan_included_volume} is the Free plan's monthly volume stop, not a rate limit
 * ({@link #isPlanStop()}): it is never retried.
 */
public final class UnavailableException extends VunlyException {
  private static final long serialVersionUID = 1L;

  UnavailableException(
      String code, int status, String message, String requestId, Map<String, Object> details, Duration retryAfter) {
    super(code, status, message, requestId, details, retryAfter);
  }

  /** True for the Free plan's monthly volume stop (never retried; {@link #retryAfter()} is the time to next month). */
  public boolean isPlanStop() {
    return status() == 429 && "plan_included_volume".equals(details().get("reason"));
  }
}
